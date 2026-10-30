import Clutter from 'gi://Clutter';
import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {METRICS, formatMetric, metricTemp} from './metrics.js';

const BUS = 'com.ikario.Termica1';
const OBJ = '/com/ikario/Termica1';
const MENU_INTERVAL = 2;    // segundos: datos completos con el menú abierto

const VERTICAL = Clutter.Orientation.VERTICAL;

function tempClass(t) {
    if (t >= 90)
        return 'termica-burn';
    if (t >= 80)
        return 'termica-hot';
    return null;
}

// Tarjeta con título, valor grande, detalle y barra opcional
class Card {
    constructor(label, withBar = false) {
        this.actor = new St.BoxLayout({orientation: VERTICAL, style_class: 'termica-card', x_expand: true});
        this.actor.add_child(new St.Label({text: label, style_class: 'termica-card-label'}));
        this.value = new St.Label({text: '—', style_class: 'termica-value'});
        this.caption = new St.Label({text: '', style_class: 'termica-caption'});
        this.actor.add_child(this.value);
        this.actor.add_child(this.caption);
        if (withBar) {
            this.bar = new St.Widget({style_class: 'termica-bar', x_expand: true});
            this.fill = new St.Widget({style_class: 'termica-bar-fill'});
            this.bar.add_child(this.fill);
            this.actor.add_child(this.bar);
            this.bar.connect('notify::width', () => this._resize());
        }
        this._frac = 0;
    }

    set(value, caption, {temp = null, frac = null} = {}) {
        this.value.text = value;
        this.caption.text = caption ?? '';
        this.value.remove_style_class_name('termica-hot');
        this.value.remove_style_class_name('termica-burn');
        const cls = temp !== null ? tempClass(temp) : null;
        if (cls)
            this.value.add_style_class_name(cls);
        if (this.bar && frac !== null) {
            this._frac = Math.max(0, Math.min(1, frac));
            this._resize();
        }
    }

    _resize() {
        this.fill.width = Math.round(this.bar.width * this._frac);
    }
}

const TermicaButton = GObject.registerClass(
class TermicaButton extends PanelMenu.Button {
    _init(ext) {
        super._init(0.5, 'Térmica', false);
        this._ext = ext;
        this._data = null;
        this._status = {};
        this._menuTimer = 0;

        // barra superior: los datos que se eligen en Preferencias
        this._settings = ext.getSettings();
        this._panelBox = new St.BoxLayout({style_class: 'panel-status-menu-box termica-panel'});
        this.add_child(this._panelBox);
        this._panelItems = [];
        this._buildPanel();
        this._settings.connectObject(
            'changed::panel-items', () => {
                this._buildPanel();
                this._syncMetricSwitches();
            },
            'changed::show-labels', () => this._buildPanel(),
            'changed::color-temps', () => this._refreshPanel(true),
            'changed::interval', () => this._startPanelTimer(),
            this);

        this._buildMenu();
        this.menu.connect('open-state-changed', (_m, open) => this._onMenuOpen(open));

        this._proxy = null;
        Gio.DBusProxy.new_for_bus(Gio.BusType.SYSTEM, Gio.DBusProxyFlags.DO_NOT_LOAD_PROPERTIES,
            null, BUS, OBJ, BUS, null, (_o, res) => {
                try {
                    this._proxy = Gio.DBusProxy.new_for_bus_finish(res);
                } catch (e) {
                    logError(e, 'Térmica');
                    return;
                }
                this._proxy.connectObject(
                    'g-signal', (_p, _s, signal) => {
                        if (signal === 'QuickChanged')
                            this._refreshStatus();
                        else
                            this._reload();
                    },
                    'notify::g-name-owner', () => this._reload(),
                    this);
                this._reload();
                this._refreshPanel();
            });

        this._panelTimer = 0;
        this._startPanelTimer();
    }

    _icon(name) {
        return Gio.icon_new_for_string(`${this._ext.path}/icons/termica-${name}-symbolic.svg`);
    }

    _buildPanel() {
        this._panelBox.destroy_all_children();
        this._panelItems = [];
        const ids = this._settings.get_strv('panel-items');
        const showLabels = this._settings.get_boolean('show-labels');
        const chosen = METRICS.filter(m => ids.includes(m.id));
        if (chosen.length === 0) {
            this._panelBox.add_child(new St.Icon({
                gicon: Gio.icon_new_for_string(`${this._ext.path}/icons/termica-symbolic.svg`),
                style_class: 'system-status-icon',
            }));
            return;
        }
        for (const m of chosen) {
            const item = new St.BoxLayout({style_class: 'termica-panel-item'});
            item.add_child(new St.Icon({gicon: this._icon(m.icon), style_class: 'system-status-icon'}));
            if (m.short && (showLabels || m.pair)) {
                item.add_child(new St.Label({text: m.short, y_align: Clutter.ActorAlign.CENTER,
                    style_class: 'termica-panel-short'}));
            }
            const label = new St.Label({text: '…', y_align: Clutter.ActorAlign.CENTER,
                style_class: 'termica-panel-label'});
            item.add_child(label);
            this._panelBox.add_child(item);
            this._panelItems.push({metric: m, label});
        }
        this._needsNvidia = chosen.some(m => !m.light);
        this._refreshPanel(true);
    }

    _syncMetricSwitches() {
        if (!this._metricSwitches)
            return;
        const ids = this._settings.get_strv('panel-items');
        for (const [id, item] of this._metricSwitches)
            item.setToggleState(ids.includes(id));
    }

    _startPanelTimer() {
        if (this._panelTimer)
            GLib.source_remove(this._panelTimer);
        this._panelTimer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT,
            this._settings.get_int('interval'), () => {
                this._refreshPanel();
                return GLib.SOURCE_CONTINUE;
            });
    }

    // ------------------------------------------------------------ menú
    _buildMenu() {
        // cabecera
        const header = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        const hbox = new St.BoxLayout({orientation: VERTICAL, style_class: 'termica-header', x_expand: true});
        this._title = new St.Label({text: 'Térmica', style_class: 'termica-title'});
        this._subtitle = new St.Label({text: '', style_class: 'termica-subtitle'});
        hbox.add_child(this._title);
        hbox.add_child(this._subtitle);
        header.add_child(hbox);
        this.menu.addMenuItem(header);

        // tarjetas
        const gridItem = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        const grid = new St.BoxLayout({orientation: VERTICAL, style_class: 'termica-grid', x_expand: true});
        this.cards = {
            cpu: new Card('PROCESADOR'),
            gpu: new Card('GRÁFICA'),
            clock: new Card('RELOJ GPU', true),
            vram: new Card('MEMORIA GPU', true),
            fanCpu: new Card('VENTILADOR CPU', true),
            fanGpu: new Card('VENTILADOR GPU', true),
            ram: new Card('MEMORIA RAM', true),
            power: new Card('POTENCIA'),
        };
        const rows = [['cpu', 'gpu'], ['clock', 'vram'], ['fanCpu', 'fanGpu'], ['ram', 'power']];
        for (const [a, b] of rows) {
            const row = new St.BoxLayout({style_class: 'termica-row', x_expand: true});
            row.add_child(this.cards[a].actor);
            row.add_child(this.cards[b].actor);
            grid.add_child(row);
        }
        gridItem.add_child(grid);
        this.menu.addMenuItem(gridItem);

        // interruptores rápidos
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._boost = new PopupMenu.PopupSwitchMenuItem('Cooler Boost', false);
        this._boost.connect('toggled', (_i, state) => this._setQuick('cooler_boost', state, this._boost));
        this._turbo = new PopupMenu.PopupSwitchMenuItem('Turbo Boost del CPU', true);
        this._turbo.connect('toggled', (_i, state) => this._setQuick('turbo', state, this._turbo));
        this.menu.addMenuItem(this._boost);
        this.menu.addMenuItem(this._turbo);

        // qué mostrar en la barra
        const sub = new PopupMenu.PopupSubMenuMenuItem('Mostrar en la barra', true);
        sub.icon.gicon = this._icon('profile');
        this._metricSwitches = new Map();
        for (const m of METRICS) {
            const item = new PopupMenu.PopupSwitchMenuItem(m.title, false);
            item.insert_child_at_index(new St.Icon({gicon: this._icon(m.icon), style_class: 'popup-menu-icon'}), 1);
            item.connect('toggled', (_i, state) => {
                const set = new Set(this._settings.get_strv('panel-items'));
                if (state)
                    set.add(m.id);
                else
                    set.delete(m.id);
                this._settings.set_strv('panel-items', METRICS.map(x => x.id).filter(id => set.has(id)));
            });
            sub.menu.addMenuItem(item);
            this._metricSwitches.set(m.id, item);
        }
        sub.menu.addAction('Más opciones…', () => this._ext.openPreferences());
        this.menu.addMenuItem(sub);
        this._syncMetricSwitches();

        // perfiles
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem('Perfiles'));
        this._profiles = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._profiles);

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this.menu.addAction('Abrir Térmica…', () => {
            Shell.AppSystem.get_default().lookup_app('com.ikario.Termica.desktop')?.activate();
        });
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this.menu.addAction('Salir de Térmica', () => this._quit(), 'application-exit-symbolic');

    }

    // Oculta el icono (desactiva la extensión) y cierra la app. El servicio sigue aplicando el perfil.
    _quit() {
        Shell.AppSystem.get_default().lookup_app('com.ikario.Termica.desktop')?.request_quit();
        const uuid = this._ext.uuid;
        GLib.idle_add(GLib.PRIORITY_DEFAULT, () => {
            Main.extensionManager.disableExtension(uuid);
            Main.notify('Térmica', 'Icono oculto. Tu perfil sigue activo. Para volver a mostrarlo abre ' +
                'la app Térmica → botón del termómetro → «Mostrar icono en la barra».');
            return GLib.SOURCE_REMOVE;
        });
    }

    // ------------------------------------------------------------ D-Bus
    _call(method, args, cb) {
        if (!this._proxy?.get_name_owner()) {
            cb?.(null, 'El servicio termicad no está corriendo');
            return;
        }
        this._proxy.call(method, args, Gio.DBusCallFlags.ALLOW_INTERACTIVE_AUTHORIZATION, 30000, null,
            (proxy, res) => {
                try {
                    const out = proxy.call_finish(res).deepUnpack();
                    cb?.(out[0] ?? null, null);
                } catch (e) {
                    if (e instanceof GLib.Error)
                        Gio.DBusError.strip_remote_error(e);
                    cb?.(null, e.message);
                }
            });
    }

    _reload() {
        if (!this._proxy?.get_name_owner()) {
            this._data = null;
            this._subtitle.text = 'Servicio apagado';
            for (const {label} of this._panelItems)
                label.text = '—';
            this._profiles.removeAll();
            return;
        }
        this._call('GetProfiles', null, (json, err) => {
            if (err)
                return;
            this._data = JSON.parse(json);
            this._renderProfiles();
        });
    }

    _renderProfiles() {
        const {active, profiles} = this._data;
        const current = profiles.find(p => p.id === active);
        this._title.text = `Térmica · ${current?.name ?? ''}`;
        this._profiles.removeAll();
        for (const p of profiles) {
            const item = new PopupMenu.PopupMenuItem(p.name);
            item.setOrnament(p.id === active ? PopupMenu.Ornament.CHECK : PopupMenu.Ornament.NONE);
            item.connect('activate', () => this._activate(p));
            this._profiles.addMenuItem(item);
        }
    }

    _activate(p) {
        this._call('Activate', new GLib.Variant('(s)', [p.id]), (warnings, err) => {
            if (err)
                Main.notify('Térmica', `No se pudo activar «${p.name}»: ${err}`);
            else if (warnings)
                Main.notify('Térmica', `«${p.name}» activado con avisos: ${warnings}`);
        });
    }

    _setQuick(key, state, item) {
        this._call('SetQuick', new GLib.Variant('(sb)', [key, state]), (_r, err) => {
            if (err) {
                item.setToggleState(!state);
                Main.notify('Térmica', err);
            }
        });
    }

    // ------------------------------------------------------------ datos
    _refreshPanel(force = false) {
        if (this.menu.isOpen && !force)
            return;  // con el menú abierto la barra se actualiza con los datos completos
        if (this._panelItems.length === 0)
            return;
        this._call(this._needsNvidia ? 'GetStatus' : 'GetTemps', null, (json, err) => {
            if (err || !json)
                return;
            this._updatePanel(JSON.parse(json));
        });
    }

    _updatePanel(s) {
        const color = this._settings.get_boolean('color-temps');
        for (const {metric, label} of this._panelItems) {
            label.text = formatMetric(metric.id, s);
            label.remove_style_class_name('termica-hot');
            label.remove_style_class_name('termica-burn');
            const t = metricTemp(metric.id, s);
            const cls = color && t !== null ? tempClass(t) : null;
            if (cls)
                label.add_style_class_name(cls);
        }
    }

    _onMenuOpen(open) {
        if (this._menuTimer) {
            GLib.source_remove(this._menuTimer);
            this._menuTimer = 0;
        }
        if (!open)
            return;
        this._reload();
        this._refreshStatus();
        this._menuTimer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, MENU_INTERVAL, () => {
            this._refreshStatus();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _refreshStatus() {
        this._call('GetStatus', null, (json, err) => {
            if (err || !json)
                return;
            const s = this._status = JSON.parse(json);
            const c = this.cards;
            this._updatePanel(s);

            c.cpu.set(`${s.cpu_temp ?? '—'}°`, `${s.pl1}/${s.pl2} W · turbo ${s.turbo ? 'sí' : 'no'}`,
                {temp: s.cpu_temp});
            if (s.gpu_awake) {
                c.gpu.set(`${s.gpu_temp ?? '—'}°`, `Uso ${s.gpu_util ?? '—'}% · ${s.gpu_pstate ?? ''}`,
                    {temp: s.gpu_temp});
                c.clock.set(`${s.gpu_clock ?? '—'}`, `MHz · memoria ${s.gpu_mem_clock ?? '—'} MHz`,
                    {frac: (s.gpu_clock ?? 0) / 2100});
                const vu = s.vram_used ?? 0, vt = s.vram_total ?? 1;
                c.vram.set(`${(vu / 1024).toFixed(1)} GB`, `de ${(vt / 1024).toFixed(0)} GB`, {frac: vu / vt});
                c.power.set(s.gpu_power !== null && s.gpu_power !== undefined ? `${s.gpu_power} W` : '—',
                    'consumo de la gráfica');
            } else {
                c.gpu.set('Dormida', 'sin uso', {});
                c.clock.set('0', 'MHz', {frac: 0});
                c.vram.set('—', 'gráfica dormida', {frac: 0});
                c.power.set('0 W', 'consumo de la gráfica');
            }
            c.fanCpu.set(s.cpu_rpm ? `${s.cpu_rpm}` : `${s.cpu_fan ?? 0}%`,
                `RPM · ${s.cpu_fan ?? 0}%`, {frac: (s.cpu_fan ?? 0) / 100});
            c.fanGpu.set(s.gpu_rpm ? `${s.gpu_rpm}` : `${s.gpu_fan ?? 0}%`,
                `RPM · ${s.gpu_fan ?? 0}%`, {frac: (s.gpu_fan ?? 0) / 100});
            if (s.ram_total)
                c.ram.set(`${s.ram_used} GB`, `de ${s.ram_total} GB`, {frac: s.ram_used / s.ram_total});

            const mode = {auto: 'automático', advanced: 'curva propia', silent: 'silencioso'}[s.fan_mode] ??
                s.fan_mode;
            this._subtitle.text = `Ventiladores en ${mode}${s.cooler_boost ? ' · Cooler Boost' : ''}`;
            this._boost.setToggleState(!!s.cooler_boost);
            this._turbo.setToggleState(!!s.turbo);
        });
    }

    destroy() {
        for (const id of [this._panelTimer, this._menuTimer]) {
            if (id)
                GLib.source_remove(id);
        }
        this._panelTimer = this._menuTimer = 0;
        this._proxy?.disconnectObject(this);
        this._proxy = null;
        this._settings?.disconnectObject(this);
        this._settings = null;
        super.destroy();
    }
});

export default class TermicaExtension extends Extension {
    enable() {
        this._button = new TermicaButton(this);
        // a la izquierda del menú del sistema (Wi‑Fi, volumen, batería)
        Main.panel.addToStatusArea(this.uuid, this._button, 0, 'right');
    }

    disable() {
        this._button?.destroy();
        this._button = null;
    }
}

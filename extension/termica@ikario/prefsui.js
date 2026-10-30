// Ventana de preferencias (se separa de prefs.js para poder probarla fuera de GNOME Shell)
import Adw from 'gi://Adw';
import Gdk from 'gi://Gdk';
import Gtk from 'gi://Gtk';

import {METRICS, DEFAULT_ITEMS} from './metrics.js';

const CSS = `
.termica-preview {
  background-color: #000;
  color: #fff;
  border-radius: 999px;
  padding: 6px 14px;
  min-height: 22px;
}
.termica-preview-value { font-weight: bold; font-feature-settings: "tnum"; }
.termica-preview-short { font-size: smaller; opacity: 0.7; }
.termica-preview-hot { color: #f8c84a; }
`;

export function buildPrefs(window, settings, path) {
    const display = Gdk.Display.get_default();
    Gtk.IconTheme.get_for_display(display).add_search_path(`${path}/icons`);
    const css = new Gtk.CssProvider();
    css.load_from_string(CSS);
    Gtk.StyleContext.add_provider_for_display(display, css, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION);

    window.set_default_size(560, 820);
    window.set_search_enabled(true);

    const page = new Adw.PreferencesPage({
        title: 'Barra superior',
        icon_name: 'termica-symbolic',
    });
    window.add(page);

    // ---- vista previa
    const previewGroup = new Adw.PreferencesGroup({
        title: 'Vista previa',
        description: 'Así se verá el icono de Térmica en la barra superior (valores de ejemplo).',
    });
    const previewBox = new Gtk.Box({
        css_classes: ['termica-preview'],
        spacing: 10,
        halign: Gtk.Align.CENTER,
        margin_top: 6,
        margin_bottom: 6,
    });
    previewGroup.add(previewBox);
    page.add(previewGroup);

    const updatePreview = () => {
        let child;
        while ((child = previewBox.get_first_child()))
            previewBox.remove(child);
        const items = settings.get_strv('panel-items');
        const showLabels = settings.get_boolean('show-labels');
        const color = settings.get_boolean('color-temps');
        const chosen = METRICS.filter(m => items.includes(m.id));
        if (chosen.length === 0) {
            previewBox.append(new Gtk.Image({icon_name: 'termica-symbolic'}));
            return;
        }
        for (const m of chosen) {
            const item = new Gtk.Box({spacing: 4});
            item.append(new Gtk.Image({icon_name: `termica-${m.icon}-symbolic`}));
            if (m.short && (showLabels || m.pair))
                item.append(new Gtk.Label({label: m.short, css_classes: ['termica-preview-short']}));
            const value = new Gtk.Label({label: m.sample, css_classes: ['termica-preview-value']});
            if (color && m.id === 'cpu-temp')
                value.add_css_class('termica-preview-hot');
            item.append(value);
            previewBox.append(item);
        }
    };

    // ---- qué mostrar
    const whatGroup = new Adw.PreferencesGroup({
        title: 'Qué mostrar',
        description: 'Activa los datos que quieres ver en la barra. Los de la gráfica NVIDIA muestran «—» ' +
            'cuando está dormida, y pedirlos la despierta un momento.',
    });
    const rows = new Map();
    for (const m of METRICS) {
        const row = new Adw.SwitchRow({
            title: m.title,
            subtitle: `Ejemplo: ${m.sample}${m.light ? '' : ' · usa la gráfica NVIDIA'}`,
        });
        row.add_prefix(new Gtk.Image({icon_name: `termica-${m.icon}-symbolic`}));
        row.connect('notify::active', () => {
            const current = new Set(settings.get_strv('panel-items'));
            if (row.active)
                current.add(m.id);
            else
                current.delete(m.id);
            // se guarda en el orden de la lista
            const ordered = METRICS.map(x => x.id).filter(id => current.has(id));
            if (ordered.join() !== settings.get_strv('panel-items').join())
                settings.set_strv('panel-items', ordered);
        });
        rows.set(m.id, row);
        whatGroup.add(row);
    }
    page.add(whatGroup);

    const syncRows = () => {
        const items = settings.get_strv('panel-items');
        for (const [id, row] of rows) {
            if (row.active !== items.includes(id))
                row.active = items.includes(id);
        }
    };

    // ---- formato
    const fmtGroup = new Adw.PreferencesGroup({title: 'Formato'});
    const labelsRow = new Adw.SwitchRow({
        title: 'Mostrar nombre corto',
        subtitle: 'Agrega «CPU», «GPU», «RAM»… junto a cada valor. Los ventiladores y el consumo ' +
            'siempre lo llevan porque comparten icono.',
    });
    settings.bind('show-labels', labelsRow, 'active', 0);
    const colorRow = new Adw.SwitchRow({
        title: 'Colorear temperaturas altas',
        subtitle: 'Amarillo desde 80 °C y rojo desde 90 °C',
    });
    settings.bind('color-temps', colorRow, 'active', 0);
    const intervalRow = Adw.SpinRow.new_with_range(1, 10, 1);
    intervalRow.title = 'Actualizar cada';
    intervalRow.subtitle = 'Segundos';
    settings.bind('interval', intervalRow, 'value', 0);
    fmtGroup.add(labelsRow);
    fmtGroup.add(colorRow);
    fmtGroup.add(intervalRow);
    page.add(fmtGroup);

    // ---- restablecer
    const resetGroup = new Adw.PreferencesGroup();
    const reset = new Adw.ButtonRow({title: 'Restablecer valores', start_icon_name: 'edit-undo-symbolic'});
    reset.connect('activated', () => {
        for (const key of ['panel-items', 'show-labels', 'color-temps', 'interval'])
            settings.reset(key);
    });
    resetGroup.add(reset);
    page.add(resetGroup);

    const onChange = () => {
        syncRows();
        updatePreview();
    };
    const id = settings.connect('changed', onChange);
    window.connect('close-request', () => {
        settings.disconnect(id);
        return false;
    });
    onChange();

    return {page, defaults: DEFAULT_ITEMS};
}

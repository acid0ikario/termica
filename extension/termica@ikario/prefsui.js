import {tr, setLanguage, language} from './i18n.js';
// Ventana de preferencias (se separa de prefs.js para poder probarla fuera de GNOME Shell)
import Adw from 'gi://Adw';
import Gdk from 'gi://Gdk';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk';

import {METRICS, DEFAULT_ITEMS} from './metrics.js';

export function buildPrefs(window, settings, path) {
    const display = Gdk.Display.get_default();
    Gtk.IconTheme.get_for_display(display).add_search_path(`${path}/icons`);

    window.set_default_size(560, 720);
    window.set_search_enabled(true);

    const page = new Adw.PreferencesPage({
        title: tr("Top bar"),
        icon_name: 'termica-symbolic',
    });
    window.add(page);

    const languageGroup = new Adw.PreferencesGroup({
        title: tr('Language'), description: tr('Restart Térmica to apply the language.'),
    });
    const codes = ['system', 'en', 'es'];
    const languageRow = new Adw.ComboRow({
        title: tr('Language'),
        model: Gtk.StringList.new(['System', 'English', 'Spanish'].map(tr)),
        selected: codes.indexOf(language()),
    });
    languageRow.connect('notify::selected', () => {
        try { setLanguage(codes[languageRow.selected]); }
        catch (e) { window.add_toast(new Adw.Toast({title: tr('Could not save language preference')})); }
    });
    languageGroup.add(languageRow);
    page.add(languageGroup);

    // ---- qué mostrar
    const whatGroup = new Adw.PreferencesGroup({
        title: tr("What to show"),
        description: tr("Enable the metrics you want in the top bar. NVIDIA metrics show “—” while the GPU is asleep; requesting them briefly wakes it up."),
    });
    const rows = new Map();
    for (const m of METRICS) {
        const row = new Adw.SwitchRow({
            title: m.title,
            subtitle: `${tr('Example:')} ${m.sample}${m.light ? '' : tr(" · uses the NVIDIA GPU")}`,
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

    // ---- profiles in the menu
    const profilesGroup = new Adw.PreferencesGroup({
        title: tr("Profiles in the menu"),
        description: tr("Choose which profiles appear in the top bar menu. The active profile is always shown."),
    });
    page.add(profilesGroup);
    const profileRows = new Map();
    const syncProfileRows = () => {
        const hidden = settings.get_strv('hidden-profiles');
        for (const [id, row] of profileRows) {
            if (row.active !== !hidden.includes(id))
                row.active = !hidden.includes(id);
        }
    };
    Gio.DBus.system.call('com.ikario.Termica1', '/com/ikario/Termica1', 'com.ikario.Termica1',
        'GetProfiles', null, new GLib.VariantType('(s)'), Gio.DBusCallFlags.NONE, 5000, null, (bus, res) => {
            let profiles;
            try {
                profiles = JSON.parse(bus.call_finish(res).deepUnpack()[0]).profiles;
            } catch {
                profilesGroup.add(new Adw.ActionRow({title: tr("The termicad service is not running")}));
                return;
            }
            for (const p of profiles) {
                const row = new Adw.SwitchRow({title: p.name});
                row.connect('notify::active', () => {
                    const hidden = new Set(settings.get_strv('hidden-profiles'));
                    if (row.active)
                        hidden.delete(p.id);
                    else
                        hidden.add(p.id);
                    const list = [...hidden];
                    if (list.sort().join() !== settings.get_strv('hidden-profiles').sort().join())
                        settings.set_strv('hidden-profiles', list);
                });
                profileRows.set(p.id, row);
                profilesGroup.add(row);
            }
            syncProfileRows();
        });

    // ---- formato
    const fmtGroup = new Adw.PreferencesGroup({title: tr("Format")});
    const labelsRow = new Adw.SwitchRow({
        title: tr("Show short labels"),
        subtitle: tr("Adds “CPU”, “GPU”, “RAM”… next to each value. Fans and power always include labels because they share icons."),
    });
    settings.bind('show-labels', labelsRow, 'active', 0);
    const colorRow = new Adw.SwitchRow({
        title: tr("Color high temperatures"),
        subtitle: tr("Yellow from 80 °C and red from 90 °C"),
    });
    settings.bind('color-temps', colorRow, 'active', 0);
    const intervalRow = Adw.SpinRow.new_with_range(1, 10, 1);
    intervalRow.title = tr("Update every");
    intervalRow.subtitle = tr("Seconds");
    settings.bind('interval', intervalRow, 'value', 0);
    fmtGroup.add(labelsRow);
    fmtGroup.add(colorRow);
    fmtGroup.add(intervalRow);
    page.add(fmtGroup);

    // ---- restablecer
    const resetGroup = new Adw.PreferencesGroup();
    const reset = new Adw.ButtonRow({title: tr("Reset defaults"), start_icon_name: 'edit-undo-symbolic'});
    reset.connect('activated', () => {
        for (const key of ['panel-items', 'show-labels', 'color-temps', 'interval', 'hidden-profiles'])
            settings.reset(key);
    });
    resetGroup.add(reset);
    page.add(resetGroup);

    const id = settings.connect('changed', () => {
        syncRows();
        syncProfileRows();
    });
    window.connect('close-request', () => {
        settings.disconnect(id);
        return false;
    });
    syncRows();

    return {page, defaults: DEFAULT_ITEMS};
}

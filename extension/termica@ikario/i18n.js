// Both the desktop app and extension use this per-user preference and catalog.
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const file = Gio.File.new_for_path(GLib.build_filenamev([GLib.get_user_config_dir(), 'termica', 'language']));
const catalogFile = Gio.File.new_for_uri(import.meta.url).get_parent().get_child('es.json');
const [, bytes] = catalogFile.load_contents(null);
const catalog = JSON.parse(new TextDecoder().decode(bytes));

export function language() {
    try {
        const [, data] = file.load_contents(null);
        const value = new TextDecoder().decode(data).trim();
        return ['en', 'es', 'system'].includes(value) ? value : 'en';
    } catch { return 'en'; }
}

export function setLanguage(value) {
    GLib.mkdir_with_parents(file.get_parent().get_path(), 0o755);
    file.replace_contents(value, null, false, Gio.FileCreateFlags.REPLACE_DESTINATION, null);
}

export function tr(text) {
    const selected = language();
    const locale = selected === 'system' ? GLib.get_language_names()[0] : selected;
    return /^es([_.@-]|$)/i.test(locale) ? (catalog[text] ?? text) : text;
}

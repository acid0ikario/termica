import {ExtensionPreferences} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import {buildPrefs} from './prefsui.js';

export default class TermicaPreferences extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        window._settings = this.getSettings();
        buildPrefs(window, window._settings, this.path);
    }
}

#!/usr/bin/python3
"""Térmica: editor de perfiles de GPU, CPU y ventiladores (GTK4 + libadwaita)."""
import copy
import json
import os
import sys

import gi

gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
gi.require_version('PangoCairo', '1.0')
from gi.repository import Adw, Gdk, Gio, GLib, GObject, Gtk, PangoCairo  # noqa: E402

APP_ID = 'com.ikario.Termica'
BUS = 'com.ikario.Termica1'
OBJ = '/com/ikario/Termica1'

EXT_DIR = os.path.expanduser('~/.local/share/gnome-shell/extensions/termica@ikario')
SCHEMA_ID = 'org.gnome.shell.extensions.termica'
EXT_UUID = 'termica@ikario'
# Igual que extension/termica@ikario/metrics.js: (id, título, icono, usa la NVIDIA)
PANEL_METRICS = [
    ('profile', 'Perfil activo', 'profile', False),
    ('cpu-temp', 'Temperatura del CPU', 'cpu', False),
    ('gpu-temp', 'Temperatura de la GPU', 'gpu', False),
    ('cpu-power', 'Consumo del CPU', 'bolt', False),
    ('gpu-power', 'Consumo de la GPU', 'bolt', True),
    ('cpu-fan-rpm', 'Ventilador 1 (CPU), RPM', 'fan', False),
    ('gpu-fan-rpm', 'Ventilador 2 (GPU), RPM', 'fan', False),
    ('cpu-fan-pct', 'Ventilador 1 (CPU), %', 'fan', False),
    ('gpu-fan-pct', 'Ventilador 2 (GPU), %', 'fan', False),
    ('gpu-clock', 'Reloj del núcleo GPU', 'speed', True),
    ('gpu-mem-clock', 'Reloj de la memoria GPU', 'wave', True),
    ('gpu-util', 'Uso de la GPU', 'activity', True),
    ('vram', 'Memoria GPU usada (VRAM)', 'vram', True),
    ('ram', 'Memoria RAM usada', 'ram', False),
]


def panel_settings():
    """Configuración de la extensión (la barra superior), o None si no está instalada."""
    schemas = os.path.join(EXT_DIR, 'schemas')
    if not os.path.exists(os.path.join(schemas, 'gschemas.compiled')):
        return None
    src = Gio.SettingsSchemaSource.new_from_directory(schemas, Gio.SettingsSchemaSource.get_default(), False)
    schema = src.lookup(SCHEMA_ID, False)
    return Gio.Settings.new_full(schema, None, None) if schema else None


class PanelDialog(Adw.PreferencesDialog):
    """Elegir qué datos se muestran junto al icono de la barra superior."""

    def __init__(self, settings):
        super().__init__(title='Barra superior', search_enabled=True)
        self.settings = settings
        page = Adw.PreferencesPage(title='Barra superior', icon_name='termica-symbolic')
        self.add(page)

        top = Adw.PreferencesGroup()
        self.visible_row = Adw.SwitchRow(title='Mostrar icono en la barra',
                                         subtitle='Al apagarlo se oculta el icono; tu perfil sigue activo')
        self.visible_row.add_prefix(Gtk.Image(icon_name='termica-symbolic'))
        self.visible_row.set_active(EXT_UUID in Gio.Settings.new('org.gnome.shell').get_strv('enabled-extensions'))
        self.visible_row.connect('notify::active', self._toggle_extension)
        top.add(self.visible_row)
        page.add(top)

        group = Adw.PreferencesGroup(
            title='Qué mostrar en la barra',
            description='Los cambios se ven al instante. Los datos de la NVIDIA muestran «—» '
                        'cuando la gráfica está dormida.')
        self.rows = {}
        for mid, title, icon, nvidia in PANEL_METRICS:
            row = Adw.SwitchRow(title=title, subtitle='Usa la gráfica NVIDIA' if nvidia else '')
            row.add_prefix(Gtk.Image(icon_name=f'termica-{icon}-symbolic'))
            row.connect('notify::active', self._toggled, mid)
            self.rows[mid] = row
            group.add(row)
        page.add(group)

        fmt = Adw.PreferencesGroup(title='Formato')
        labels = Adw.SwitchRow(title='Mostrar nombre corto', subtitle='«CPU», «GPU», «RAM»… junto a cada valor')
        settings.bind('show-labels', labels, 'active', Gio.SettingsBindFlags.DEFAULT)
        color = Adw.SwitchRow(title='Colorear temperaturas altas', subtitle='Amarillo desde 80 °C, rojo desde 90 °C')
        settings.bind('color-temps', color, 'active', Gio.SettingsBindFlags.DEFAULT)
        interval = Adw.SpinRow.new_with_range(1, 10, 1)
        interval.set_title('Actualizar cada')
        interval.set_subtitle('Segundos')
        settings.bind('interval', interval, 'value', Gio.SettingsBindFlags.DEFAULT)
        for w in (labels, color, interval):
            fmt.add(w)
        page.add(fmt)

        self._sync()
        self._sid = settings.connect('changed::panel-items', lambda *_: self._sync())
        self.connect('closed', lambda *_: settings.disconnect(self._sid))

    def _toggle_extension(self, row, _pspec):
        method = 'EnableExtension' if row.get_active() else 'DisableExtension'
        Gio.DBus.session.call('org.gnome.Shell.Extensions', '/org/gnome/Shell/Extensions',
                              'org.gnome.Shell.Extensions', method, GLib.Variant('(s)', (EXT_UUID,)),
                              None, Gio.DBusCallFlags.NONE, -1, None, None)

    def _sync(self):
        items = self.settings.get_strv('panel-items')
        for mid, row in self.rows.items():
            if row.get_active() != (mid in items):
                row.set_active(mid in items)

    def _toggled(self, row, _pspec, mid):
        items = set(self.settings.get_strv('panel-items'))
        items.add(mid) if row.get_active() else items.discard(mid)
        ordered = [m[0] for m in PANEL_METRICS if m[0] in items]
        if ordered != self.settings.get_strv('panel-items'):
            self.settings.set_strv('panel-items', ordered)


MEM_OPTIONS = [(None, 'De fábrica (6001 MHz)'), (5501, '5501 MHz'), (810, '810 MHz')]
FAN_OPTIONS = [('auto', 'Fábrica (automático)'), ('advanced', 'Curva personalizada')]
SHIFT_OPTIONS = [(None, 'No cambiar'), ('eco', 'Eco'), ('comfort', 'Comfort'), ('turbo', 'Turbo')]
SPEED_MAX = 150
LAST_SPEED_MIN = 70

CSS = """
.stat { padding: 12px 16px; }
.stat-value { font-size: 20pt; font-weight: 800; font-feature-settings: "tnum"; }
.stat-hot .stat-value { color: var(--warning-color); }
.stat-burn .stat-value { color: var(--error-color); }
.hero-title { font-size: 22pt; font-weight: 800; }
.curve-card { padding: 6px; }
"""


def index_of(options, value):
    keys = [x[0] for x in options]
    return keys.index(value) if value in keys else 0


def summary(p):
    parts = []
    core = p['gpu']['core_max']
    parts.append(f'GPU ≤ {core} MHz' if core else 'GPU de fábrica')
    parts.append(f'CPU {p["cpu"]["pl1"]}/{p["cpu"]["pl2"]} W')
    parts.append('curva propia' if p['fans']['mode'] == 'advanced' else 'ventilador auto')
    if not p['cpu'].get('turbo', True):
        parts.append('sin turbo')
    if p['fans']['cooler_boost']:
        parts.append('cooler boost')
    return ' · '.join(parts)


# ------------------------------------------------------------------ D-Bus

class Client(GObject.Object):
    __gsignals__ = {
        'active-changed': (GObject.SignalFlags.RUN_FIRST, None, (str,)),
        'profiles-changed': (GObject.SignalFlags.RUN_FIRST, None, ()),
        'available': (GObject.SignalFlags.RUN_FIRST, None, (bool,)),
    }

    def __init__(self):
        super().__init__()
        self.proxy = Gio.DBusProxy.new_for_bus_sync(
            Gio.BusType.SESSION if os.environ.get('TERMICA_DEV') else Gio.BusType.SYSTEM,
            Gio.DBusProxyFlags.DO_NOT_LOAD_PROPERTIES, None, BUS, OBJ, BUS, None)
        self.proxy.connect('g-signal', self._on_signal)
        self.proxy.connect('notify::g-name-owner', lambda p, _: self.emit('available', self.is_available()))

    def is_available(self):
        return self.proxy.get_name_owner() is not None

    def _on_signal(self, _proxy, _sender, signal, params):
        if signal == 'ActiveChanged':
            self.emit('active-changed', params.unpack()[0])
        elif signal == 'ProfilesChanged':
            self.emit('profiles-changed')

    def call(self, method, args=None, callback=None):
        variant = GLib.Variant('(s)', (args,)) if args is not None else None

        def done(proxy, res):
            try:
                out = proxy.call_finish(res).unpack()
                result, error = (out[0] if out else None), None
            except GLib.Error as e:
                Gio.DBusError.strip_remote_error(e)
                result, error = None, e.message
            if callback:
                callback(result, error)

        self.proxy.call(method, variant, Gio.DBusCallFlags.ALLOW_INTERACTIVE_AUTHORIZATION, 30000, None, done)


# ------------------------------------------------------------------ curva

class CurveEditor(Gtk.DrawingArea):
    """Curva escalonada del EC: 7 velocidades y 6 temperaturas de cambio."""
    __gsignals__ = {'changed': (GObject.SignalFlags.RUN_FIRST, None, ())}
    T_MIN, T_MAX = 30, 100
    PAD_L, PAD_R, PAD_T, PAD_B = 46, 18, 16, 30

    def __init__(self):
        super().__init__()
        self.set_content_height(280)
        self.set_hexpand(True)
        self.temps = [60, 70, 76, 82, 86, 90]
        self.speeds = [0, 30, 38, 50, 65, 85, 110]
        self.current = None
        self.hover = None
        self.drag = None
        self.set_draw_func(self._draw)

        drag = Gtk.GestureDrag()
        drag.connect('drag-begin', self._drag_begin)
        drag.connect('drag-update', self._drag_update)
        drag.connect('drag-end', self._drag_end)
        self.add_controller(drag)
        motion = Gtk.EventControllerMotion()
        motion.connect('motion', self._motion)
        motion.connect('leave', lambda *_: self._set_hover(None))
        self.add_controller(motion)

        sm = Adw.StyleManager.get_default()
        sm.connect('notify::dark', lambda *_: self.queue_draw())
        sm.connect('notify::accent-color-rgba', lambda *_: self.queue_draw())

    # geometría
    def _box(self):
        return self.PAD_L, self.PAD_T, self.get_width() - self.PAD_R, self.get_height() - self.PAD_B

    def _tx(self, t):
        x0, _, x1, _ = self._box()
        return x0 + (t - self.T_MIN) / (self.T_MAX - self.T_MIN) * (x1 - x0)

    def _sy(self, s):
        _, y0, _, y1 = self._box()
        return y1 - s / SPEED_MAX * (y1 - y0)

    def _inv_t(self, x):
        x0, _, x1, _ = self._box()
        return self.T_MIN + (x - x0) / max(1, x1 - x0) * (self.T_MAX - self.T_MIN)

    def _inv_s(self, y):
        _, y0, _, y1 = self._box()
        return (y1 - y) / max(1, y1 - y0) * SPEED_MAX

    def _points(self):
        """(índice, x, y). Índice -1 = velocidad base (antes del primer umbral)."""
        pts = [(-1, self._tx(self.T_MIN), self._sy(self.speeds[0]))]
        pts += [(i, self._tx(t), self._sy(self.speeds[i + 1])) for i, t in enumerate(self.temps)]
        return pts

    def set_curve(self, curve):
        self.temps = list(curve['temps'])
        self.speeds = list(curve['speeds'])
        self.queue_draw()

    def get_curve(self):
        return {'temps': list(self.temps), 'speeds': list(self.speeds)}

    def set_current(self, temp):
        if temp != self.current:
            self.current = temp
            self.queue_draw()

    # interacción
    def _nearest(self, x, y):
        best, dist = None, 18 ** 2
        for i, px, py in self._points():
            d = (px - x) ** 2 + (py - y) ** 2
            if d < dist:
                best, dist = i, d
        return best

    def _set_hover(self, idx):
        if idx != self.hover:
            self.hover = idx
            self.set_cursor_from_name('grab' if idx is not None else None)
            self.queue_draw()

    def _motion(self, _c, x, y):
        if self.drag is None:
            self._set_hover(self._nearest(x, y))

    def _drag_begin(self, gesture, x, y):
        idx = self._nearest(x, y)
        if idx is None:
            gesture.set_state(Gtk.EventSequenceState.DENIED)
            return
        self.drag = (idx, x, y)
        self.set_cursor_from_name('grabbing')

    def _drag_update(self, _g, dx, dy):
        if self.drag is None:
            return
        idx, sx, sy = self.drag
        x, y = sx + dx, sy + dy
        si = idx + 1  # índice de la velocidad que controla este punto
        lo_s = self.speeds[si - 1] if si > 0 else 0
        hi_s = self.speeds[si + 1] if si < 6 else SPEED_MAX
        if si == 6:
            lo_s = max(lo_s, LAST_SPEED_MIN)
        s = int(round(self._inv_s(y)))
        self.speeds[si] = max(lo_s, min(hi_s, s))
        if idx >= 0:
            lo_t = self.temps[idx - 1] + 1 if idx > 0 else self.T_MIN
            hi_t = self.temps[idx + 1] - 1 if idx < 5 else self.T_MAX
            t = int(round(self._inv_t(x)))
            self.temps[idx] = max(lo_t, min(hi_t, t))
        self.hover = idx
        self.queue_draw()
        self.emit('changed')

    def _drag_end(self, *_):
        self.drag = None
        self.set_cursor_from_name('grab' if self.hover is not None else None)

    # dibujo
    def _text(self, cr, text, x, y, color, align='left', alpha=1.0):
        layout = self.create_pango_layout(text)
        w, h = layout.get_pixel_size()
        if align == 'center':
            x -= w / 2
        elif align == 'right':
            x -= w
        cr.set_source_rgba(color.red, color.green, color.blue, alpha)
        cr.move_to(x, y - h / 2)
        PangoCairo.show_layout(cr, layout)
        return w, h

    def _draw(self, _area, cr, width, height):
        fg = self.get_color()
        accent = Adw.StyleManager.get_default().get_accent_color_rgba()
        x0, y0, x1, y1 = self._box()

        # rejilla y ejes
        cr.set_line_width(1)
        for s in range(0, SPEED_MAX + 1, 25):
            y = self._sy(s)
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.10)
            cr.move_to(x0, y); cr.line_to(x1, y); cr.stroke()
            self._text(cr, f'{s}%', x0 - 8, y, fg, 'right', 0.55)
        for t in range(self.T_MIN, self.T_MAX + 1, 10):
            x = self._tx(t)
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.06)
            cr.move_to(x, y0); cr.line_to(x, y1); cr.stroke()
            self._text(cr, f'{t}°', x, y1 + 16, fg, 'center', 0.55)

        # zona peligrosa (>= 90 °C)
        cr.set_source_rgba(0.88, 0.11, 0.14, 0.07)
        cr.rectangle(self._tx(90), y0, x1 - self._tx(90), y1 - y0)
        cr.fill()

        # curva escalonada
        def path():
            cr.move_to(x0, self._sy(self.speeds[0]))
            for i, t in enumerate(self.temps):
                x = self._tx(t)
                cr.line_to(x, self._sy(self.speeds[i]))
                cr.line_to(x, self._sy(self.speeds[i + 1]))
            cr.line_to(x1, self._sy(self.speeds[6]))

        path()
        cr.line_to(x1, y1); cr.line_to(x0, y1); cr.close_path()
        cr.set_source_rgba(accent.red, accent.green, accent.blue, 0.16)
        cr.fill()
        path()
        cr.set_source_rgba(accent.red, accent.green, accent.blue, 1)
        cr.set_line_width(2.5)
        cr.set_line_join(1)  # ROUND
        cr.stroke()

        # temperatura actual
        if self.current is not None and self.T_MIN <= self.current <= self.T_MAX:
            x = self._tx(self.current)
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.65)
            cr.set_dash([4, 4])
            cr.set_line_width(1.5)
            cr.move_to(x, y0); cr.line_to(x, y1); cr.stroke()
            cr.set_dash([])
            self._text(cr, f'Ahora {self.current}°', min(x + 6, x1 - 70), y0 + 8, fg, 'left', 0.8)

        # puntos
        bg = Gdk.RGBA()
        bg.parse('#ffffff' if not Adw.StyleManager.get_default().get_dark() else '#1e1e1e')
        for i, px, py in self._points():
            r = 8 if i == self.hover else 6
            cr.arc(px, py, r + 2.5, 0, 6.2832)
            cr.set_source_rgba(bg.red, bg.green, bg.blue, 1)
            cr.fill()
            cr.arc(px, py, r, 0, 6.2832)
            cr.set_source_rgba(accent.red, accent.green, accent.blue, 1)
            cr.fill()

        # etiqueta del punto activo
        if self.hover is not None:
            i = self.hover
            px, py = [(x, y) for j, x, y in self._points() if j == i][0]
            label = (f'Base: {self.speeds[0]}%' if i < 0 else f'Desde {self.temps[i]}° → {self.speeds[i + 1]}%')
            layout = self.create_pango_layout(label)
            w, h = layout.get_pixel_size()
            bx = min(max(px - w / 2 - 8, x0), x1 - w - 16)
            by = py - h - 22 if py - h - 22 > 2 else py + 14
            cr.set_source_rgba(fg.red, fg.green, fg.blue, 0.92)
            _rounded(cr, bx, by, w + 16, h + 8, 6)
            cr.fill()
            cr.set_source_rgba(bg.red, bg.green, bg.blue, 1)
            cr.move_to(bx + 8, by + 4)
            PangoCairo.show_layout(cr, layout)


def _rounded(cr, x, y, w, h, r):
    cr.new_sub_path()
    cr.arc(x + w - r, y + r, r, -1.5708, 0)
    cr.arc(x + w - r, y + h - r, r, 0, 1.5708)
    cr.arc(x + r, y + h - r, r, 1.5708, 3.1416)
    cr.arc(x + r, y + r, r, 3.1416, 4.7124)
    cr.close_path()


# ------------------------------------------------------------------ tarjetas de estado

class Stat(Gtk.Box):
    def __init__(self, caption):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        self.add_css_class('card')
        self.add_css_class('stat')
        self.set_hexpand(True)
        self.value = Gtk.Label(label='—', xalign=0, css_classes=['stat-value'])
        self.detail = Gtk.Label(label=caption, xalign=0, css_classes=['dim-label', 'caption'])
        self.append(self.value)
        self.append(self.detail)

    def set(self, value, detail=None, temp=None):
        self.value.set_label(value)
        if detail is not None:
            self.detail.set_label(detail)
        self.remove_css_class('stat-hot')
        self.remove_css_class('stat-burn')
        if temp is not None and temp >= 90:
            self.add_css_class('stat-burn')
        elif temp is not None and temp >= 80:
            self.add_css_class('stat-hot')


# ------------------------------------------------------------------ ventana

class Window(Adw.ApplicationWindow):
    def __init__(self, app, client):
        super().__init__(application=app, title='Térmica', default_width=1040, default_height=760)
        self.set_size_request(360, 480)
        self.client = client
        self.data = None
        self.selected = None      # id del perfil mostrado
        self.dirty = False
        self.loading = False
        self.curves = {}
        self.curve_key = 'cpu'
        self.status = {}

        self.toasts = Adw.ToastOverlay()
        self.split = Adw.NavigationSplitView(min_sidebar_width=250, max_sidebar_width=330)
        self.toasts.set_child(self.split)

        self.stack = Gtk.Stack(transition_type=Gtk.StackTransitionType.CROSSFADE)
        self.stack.add_named(self.toasts, 'main')
        self.stack.add_named(self._build_unavailable(), 'unavailable')
        self.set_content(self.stack)

        self.split.set_sidebar(self._build_sidebar())
        self.split.set_content(self._build_content())

        bp = Adw.Breakpoint.new(Adw.BreakpointCondition.parse('max-width: 680sp'))
        bp.add_setter(self.split, 'collapsed', True)
        self.add_breakpoint(bp)

        client.connect('active-changed', lambda *_: self.reload())
        client.connect('profiles-changed', lambda *_: self.reload())
        client.connect('available', lambda _c, ok: self._on_available(ok))
        self._on_available(client.is_available())

        self._poll_id = GLib.timeout_add(2000, self._poll)
        self.connect('close-request', self._on_close)

    # --- construcción
    def _build_unavailable(self):
        page = Adw.StatusPage(icon_name='dialog-warning-symbolic', title='El servicio no está corriendo',
                              description='Térmica necesita el servicio termicad.\n'
                                          'Ejecútalo con: sudo systemctl start termicad')
        tv = Adw.ToolbarView(content=page)
        tv.add_top_bar(Adw.HeaderBar())
        return tv

    def _build_sidebar(self):
        header = Adw.HeaderBar()
        add = Gtk.Button(icon_name='list-add-symbolic', tooltip_text='Nuevo perfil')
        add.connect('clicked', lambda *_: self._new_profile(duplicate=False))
        header.pack_start(add)
        bar = Gtk.Button(icon_name='termica-symbolic', tooltip_text='Qué mostrar en la barra superior')
        bar.connect('clicked', lambda *_: self._open_panel_dialog())
        header.pack_end(bar)

        self.listbox = Gtk.ListBox(css_classes=['navigation-sidebar'])
        self.listbox.connect('row-activated', self._on_row_activated)

        stats = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, margin_start=12, margin_end=12,
                        margin_bottom=12, margin_top=6)
        stats.append(Gtk.Label(label='Ahora mismo', xalign=0, css_classes=['heading']))
        grid = Gtk.Grid(column_spacing=8, row_spacing=8, column_homogeneous=True)
        self.st_cpu, self.st_gpu = Stat('Procesador'), Stat('Gráfica')
        self.st_fan, self.st_clock = Stat('Ventiladores'), Stat('Núcleo GPU')
        grid.attach(self.st_cpu, 0, 0, 1, 1)
        grid.attach(self.st_gpu, 1, 0, 1, 1)
        grid.attach(self.st_fan, 0, 1, 1, 1)
        grid.attach(self.st_clock, 1, 1, 1, 1)
        stats.append(grid)

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        scroll = Gtk.ScrolledWindow(vexpand=True, hscrollbar_policy=Gtk.PolicyType.NEVER, child=self.listbox)
        box.append(scroll)
        box.append(stats)

        tv = Adw.ToolbarView(content=box)
        tv.add_top_bar(header)
        return Adw.NavigationPage(title='Térmica', child=tv)

    def _build_content(self):
        header = Adw.HeaderBar()
        self.save_btn = Gtk.Button(label='Guardar', css_classes=['suggested-action'], sensitive=False)
        self.save_btn.connect('clicked', lambda *_: self._save())
        header.pack_end(self.save_btn)

        page = Adw.PreferencesPage()

        # cabecera del perfil
        hero = Adw.PreferencesGroup()
        hbox = Gtk.Box(spacing=12, margin_bottom=6)
        tbox = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, hexpand=True, valign=Gtk.Align.CENTER)
        self.hero_title = Gtk.Label(xalign=0, css_classes=['hero-title'], ellipsize=3)
        self.hero_sub = Gtk.Label(xalign=0, css_classes=['dim-label'], wrap=True)
        tbox.append(self.hero_title)
        tbox.append(self.hero_sub)
        self.activate_btn = Gtk.Button(label='Activar', css_classes=['pill', 'suggested-action'],
                                       valign=Gtk.Align.CENTER)
        self.activate_btn.connect('clicked', lambda *_: self._activate())
        hbox.append(tbox)
        hbox.append(self.activate_btn)
        hero.add(hbox)
        page.add(hero)

        # general
        g = Adw.PreferencesGroup(title='General')
        self.name_row = Adw.EntryRow(title='Nombre del perfil')
        self.name_row.connect('changed', self._changed)
        g.add(self.name_row)
        page.add(g)

        # GPU
        g = Adw.PreferencesGroup(title='Gráfica NVIDIA',
                                 description='RTX 3060 Laptop. De fábrica sube hasta 2100 MHz. '
                                             'Limitarla baja el calor y el ruido, pero también la velocidad '
                                             'en juegos y renders de Blender.')
        self.core_switch = Adw.SwitchRow(title='Limitar velocidad del núcleo')
        self.core_spin = Adw.SpinRow.new_with_range(300, 2100, 50)
        self.core_spin.set_title('Velocidad máxima del núcleo')
        self.core_spin.set_subtitle('MHz')
        self.core_switch.bind_property('active', self.core_spin, 'sensitive', GObject.BindingFlags.SYNC_CREATE)
        self.mem_combo = Adw.ComboRow(title='Memoria máxima', subtitle='Bajarla casi no reduce el calor',
                                      model=Gtk.StringList.new([x[1] for x in MEM_OPTIONS]))
        for w, sig in ((self.core_switch, 'notify::active'), (self.core_spin, 'notify::value'),
                       (self.mem_combo, 'notify::selected')):
            w.connect(sig, self._changed)
            g.add(w)
        page.add(g)

        # CPU
        g = Adw.PreferencesGroup(title='Procesador',
                                 description='Intel i7-1260P. De fábrica: 80 W sostenido y 200 W en ráfagas.')
        self.pl1 = Adw.SpinRow.new_with_range(5, 115, 1)
        self.pl1.set_title('Potencia sostenida (PL1)')
        self.pl1.set_subtitle('Vatios que puede usar todo el tiempo')
        self.pl2 = Adw.SpinRow.new_with_range(5, 200, 1)
        self.pl2.set_title('Potencia de ráfaga (PL2)')
        self.pl2.set_subtitle('Vatios por unos segundos al arrancar una tarea')
        for w in (self.pl1, self.pl2):
            w.connect('notify::value', self._changed)
            g.add(w)
        self.turbo = Adw.SwitchRow(title='Turbo Boost',
                                   subtitle='Apagarlo baja mucho el calor; el CPU queda en su velocidad base')
        self.turbo.connect('notify::active', self._changed)
        g.add(self.turbo)
        page.add(g)

        # ventiladores
        g = Adw.PreferencesGroup(title='Ventiladores')
        self.fan_combo = Adw.ComboRow(title='Control', model=Gtk.StringList.new([x[1] for x in FAN_OPTIONS]))
        self.boost = Adw.SwitchRow(title='Cooler Boost', subtitle='Ventiladores al máximo todo el tiempo')
        self.shift_combo = Adw.ComboRow(title='Modo del firmware MSI',
                                        subtitle='Ajuste interno del portátil (Eco, Comfort o Turbo)',
                                        model=Gtk.StringList.new([x[1] for x in SHIFT_OPTIONS]))
        for w, sig in ((self.fan_combo, 'notify::selected'), (self.boost, 'notify::active'),
                       (self.shift_combo, 'notify::selected')):
            w.connect(sig, self._changed)
            g.add(w)
        page.add(g)

        # curva
        self.curve_group = Adw.PreferencesGroup(
            title='Curva de ventilador',
            description='Arrastra los puntos: a la derecha cambia la temperatura y hacia arriba la velocidad. '
                        'La línea punteada es la temperatura actual.')
        toggles = Adw.ToggleGroup(valign=Gtk.Align.CENTER)
        toggles.add(Adw.Toggle(name='cpu', label='CPU'))
        toggles.add(Adw.Toggle(name='gpu', label='GPU'))
        toggles.set_active_name('cpu')
        toggles.connect('notify::active-name', self._on_curve_key)
        self.curve_group.set_header_suffix(toggles)
        self.curve = CurveEditor()
        self.curve.connect('changed', self._on_curve_changed)
        frame = Gtk.Box(css_classes=['card', 'curve-card'])
        frame.append(self.curve)
        self.curve_group.add(frame)
        page.add(self.curve_group)

        # acciones
        g = Adw.PreferencesGroup()
        dup = Adw.ButtonRow(title='Duplicar perfil', start_icon_name='edit-copy-symbolic')
        dup.connect('activated', lambda *_: self._new_profile(duplicate=True))
        self.delete_row = Adw.ButtonRow(title='Eliminar perfil', start_icon_name='user-trash-symbolic',
                                        css_classes=['destructive-action'])
        self.delete_row.connect('activated', lambda *_: self._confirm_delete())
        g.add(dup)
        g.add(self.delete_row)
        page.add(g)

        tv = Adw.ToolbarView(content=page)
        tv.add_top_bar(header)
        self.content_page = Adw.NavigationPage(title='Perfil', child=tv)
        return self.content_page

    def _open_panel_dialog(self):
        settings = panel_settings()
        if not settings:
            self.toast('La extensión de la barra no está instalada')
            return
        PanelDialog(settings).present(self)

    # --- datos
    def _on_available(self, ok):
        self.stack.set_visible_child_name('main' if ok else 'unavailable')
        if ok:
            self.reload()
            self._poll()

    def reload(self, select=None):
        def done(result, error):
            if error:
                self.toast(error)
                return
            self.data = json.loads(result)
            target = select or self.selected or self.data['active']
            ids = [p['id'] for p in self.data['profiles']]
            if target not in ids:
                target = self.data['active']
            self._fill_list()
            if not self.dirty or select:
                self._show(target)
            else:
                self._update_hero()
        self.client.call('GetProfiles', callback=done)

    def _profile(self, pid):
        return next(p for p in self.data['profiles'] if p['id'] == pid)

    def _fill_list(self):
        self.listbox.remove_all()
        for p in self.data['profiles']:
            row = Adw.ActionRow(title=GLib.markup_escape_text(p['name']), subtitle=summary(p),
                                activatable=True, subtitle_lines=2)
            row.profile_id = p['id']
            icon = Gtk.Image(icon_name='object-select-symbolic', visible=p['id'] == self.data['active'],
                             tooltip_text='Perfil activo')
            row.add_suffix(icon)
            self.listbox.append(row)
            if p['id'] == self.selected:
                self.listbox.select_row(row)

    def _show(self, pid):
        p = self._profile(pid)
        self.selected = pid
        self.loading = True
        self.name_row.set_text(p['name'])
        core = p['gpu']['core_max']
        self.core_switch.set_active(core is not None)
        self.core_spin.set_value(core or 1400)
        self.mem_combo.set_selected(index_of(MEM_OPTIONS, p['gpu']['mem_max']))
        self.pl1.set_value(p['cpu']['pl1'])
        self.pl2.set_value(p['cpu']['pl2'])
        self.turbo.set_active(p['cpu'].get('turbo', True))
        self.fan_combo.set_selected(index_of(FAN_OPTIONS, p['fans']['mode']))
        self.boost.set_active(p['fans']['cooler_boost'])
        self.shift_combo.set_selected(index_of(SHIFT_OPTIONS, p.get('shift_mode')))
        self.curves = copy.deepcopy(p['fans']['curves'])
        self.curve.set_curve(self.curves[self.curve_key])
        self.loading = False
        self._set_dirty(False)
        self._update_hero()
        for row in self._rows():
            if row.profile_id == pid:
                self.listbox.select_row(row)

    def _rows(self):
        child = self.listbox.get_first_child()
        while child:
            yield child
            child = child.get_next_sibling()

    def _update_hero(self):
        if not self.data or not self.selected:
            return
        active = self.selected == self.data['active']
        self.hero_title.set_label(self.name_row.get_text() or 'Sin nombre')
        self.hero_sub.set_label('Este es el perfil activo' if active else 'Perfil guardado, no está activo')
        self.content_page.set_title(self.name_row.get_text() or 'Perfil')
        if self.dirty:
            self.activate_btn.set_label('Guardar y activar' if not active else 'Guardar y aplicar')
            self.activate_btn.set_sensitive(True)
        else:
            self.activate_btn.set_label('Activo' if active else 'Activar')
            self.activate_btn.set_sensitive(not active)
        self.delete_row.set_sensitive(not active and len(self.data['profiles']) > 1)
        self.curve_group.set_visible(FAN_OPTIONS[self.fan_combo.get_selected()][0] == 'advanced')

    def _collect(self):
        return {
            'id': self.selected,
            'name': self.name_row.get_text().strip(),
            'gpu': {'core_max': int(self.core_spin.get_value()) if self.core_switch.get_active() else None,
                    'mem_max': MEM_OPTIONS[self.mem_combo.get_selected()][0]},
            'cpu': {'pl1': int(self.pl1.get_value()), 'pl2': int(self.pl2.get_value()),
                    'turbo': self.turbo.get_active()},
            'fans': {'mode': FAN_OPTIONS[self.fan_combo.get_selected()][0],
                     'cooler_boost': self.boost.get_active(),
                     'curves': copy.deepcopy(self.curves)},
            'shift_mode': SHIFT_OPTIONS[self.shift_combo.get_selected()][0],
        }

    # --- eventos
    def _changed(self, *_):
        if self.loading:
            return
        if self.pl2.get_value() < self.pl1.get_value():
            self.pl2.set_value(self.pl1.get_value())
        self._set_dirty(True)

    def _set_dirty(self, dirty):
        self.dirty = dirty
        self.save_btn.set_sensitive(dirty)
        self._update_hero()

    def _on_curve_key(self, toggles, _pspec):
        self.curve_key = toggles.get_active_name()
        self.curve.set_curve(self.curves[self.curve_key])
        self._update_curve_current()

    def _on_curve_changed(self, _w):
        self.curves[self.curve_key] = self.curve.get_curve()
        self._changed()

    def _on_row_activated(self, _lb, row):
        if row.profile_id == self.selected:
            self.split.set_show_content(True)
            return
        if not self.dirty:
            self._show(row.profile_id)
            self.split.set_show_content(True)
            return
        dialog = Adw.AlertDialog(heading='¿Descartar cambios?',
                                 body=f'Tienes cambios sin guardar en «{self.name_row.get_text()}».')
        dialog.add_response('cancel', 'Cancelar')
        dialog.add_response('discard', 'Descartar')
        dialog.set_response_appearance('discard', Adw.ResponseAppearance.DESTRUCTIVE)

        def on_response(_d, response):
            if response == 'discard':
                self._set_dirty(False)
                self._show(row.profile_id)
                self.split.set_show_content(True)
            else:
                self._show_selected_row()
        dialog.connect('response', on_response)
        dialog.present(self)

    def _show_selected_row(self):
        for row in self._rows():
            if row.profile_id == self.selected:
                self.listbox.select_row(row)

    # --- acciones
    def _save(self, then=None):
        def done(result, error):
            if error:
                self.toast(error)
                return
            out = json.loads(result)
            self._set_dirty(False)
            self.toast('Guardado' + (': ' + '; '.join(out['warnings']) if out['warnings'] else ''))
            self.reload(select=out['id'])
            if then:
                then(out['id'])
        self.client.call('SaveProfile', json.dumps(self._collect()), done)

    def _activate(self):
        def do(pid):
            def done(result, error):
                if error:
                    self.toast(error)
                else:
                    self.toast(f'Perfil «{self._profile(pid)["name"]}» activado' +
                               (f'. Avisos: {result}' if result else ''))
            self.client.call('Activate', pid, done)
        if self.dirty:
            self._save(then=do)
        else:
            do(self.selected)

    def _new_profile(self, duplicate):
        if not self.data:
            return
        base = self._collect() if (duplicate and self.selected) else copy.deepcopy(self._profile(self.data['active']))
        base['id'] = None
        base['name'] = (base['name'] + ' (copia)')[:40] if duplicate else 'Nuevo perfil'

        def done(result, error):
            if error:
                self.toast(error)
                return
            out = json.loads(result)
            self.dirty = False
            self.reload(select=out['id'])
            self.split.set_show_content(True)
            self.toast('Perfil creado')
        self.client.call('SaveProfile', json.dumps(base), done)

    def _confirm_delete(self):
        name = self.name_row.get_text()
        dialog = Adw.AlertDialog(heading=f'¿Eliminar «{name}»?', body='Esta acción no se puede deshacer.')
        dialog.add_response('cancel', 'Cancelar')
        dialog.add_response('delete', 'Eliminar')
        dialog.set_response_appearance('delete', Adw.ResponseAppearance.DESTRUCTIVE)

        def on_response(_d, response):
            if response != 'delete':
                return
            pid = self.selected

            def done(_r, error):
                if error:
                    self.toast(error)
                    return
                self.selected = None
                self.dirty = False
                self.reload(select=self.data['active'])
                self.toast(f'«{name}» eliminado')
            self.client.call('DeleteProfile', pid, done)
        dialog.connect('response', on_response)
        dialog.present(self)

    # --- estado en vivo
    def _poll(self):
        if not self.client.is_available():
            return True

        def done(result, error):
            if error or not result:
                return
            s = self.status = json.loads(result)
            self.st_cpu.set(f'{s["cpu_temp"]}°' if s.get('cpu_temp') else '—',
                            f'Procesador · {s["pl1"]}/{s["pl2"]} W', s.get('cpu_temp'))
            if s.get('gpu_awake'):
                pw = f' · {s["gpu_power"]} W' if s.get('gpu_power') is not None else ''
                self.st_gpu.set(f'{s["gpu_temp"]}°' if s.get('gpu_temp') else '—', f'Gráfica{pw}',
                                s.get('gpu_temp'))
                self.st_clock.set(f'{s.get("gpu_clock", "—")}', f'MHz núcleo · {s.get("gpu_pstate", "")}')
            else:
                self.st_gpu.set('Dormida', 'Gráfica sin uso')
                self.st_clock.set('0', 'MHz núcleo')
            mode = {'auto': 'auto', 'advanced': 'curva', 'silent': 'silent'}.get(s.get('fan_mode'), s.get('fan_mode'))
            boost = ' · boost' if s.get('cooler_boost') else ''
            self.st_fan.set(f'{s.get("cpu_fan", 0)}%', f'CPU · GPU {s.get("gpu_fan", 0)}% · {mode}{boost}')
            self._update_curve_current()
        self.client.call('GetStatus', callback=done)
        return True

    def _update_curve_current(self):
        key = 'cpu_temp' if self.curve_key == 'cpu' else 'gpu_temp'
        self.curve.set_current(self.status.get(key))

    def toast(self, text):
        self.toasts.add_toast(Adw.Toast(title=GLib.markup_escape_text(text), timeout=4))

    def _on_close(self, *_):
        if self._poll_id:
            GLib.source_remove(self._poll_id)
            self._poll_id = 0
        return False


class App(Adw.Application):
    def __init__(self):
        super().__init__(application_id=APP_ID, flags=Gio.ApplicationFlags.DEFAULT_FLAGS)
        self.client = None
        self._open_bar = '--barra' in sys.argv

    def do_startup(self):
        Adw.Application.do_startup(self)
        Gtk.IconTheme.get_for_display(Gdk.Display.get_default()).add_search_path(os.path.join(EXT_DIR, 'icons'))
        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(Gdk.Display.get_default(), provider,
                                                  Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)
        quit_action = Gio.SimpleAction.new('quit', None)
        quit_action.connect('activate', lambda *_: self.quit())
        self.add_action(quit_action)
        self.set_accels_for_action('app.quit', ['<Control>q'])
        self.set_accels_for_action('window.close', ['<Control>w'])

    def do_activate(self):
        win = self.get_active_window()
        if not win:
            self.client = self.client or Client()
            win = Window(self, self.client)
            shot = os.environ.get('TERMICA_SCREENSHOT')
            if shot:
                win.set_default_size(1040, int(os.environ.get('TERMICA_SHOT_H', 760)))
                GLib.timeout_add(2500, lambda: _screenshot(win, shot))
        win.present()
        if self._open_bar:
            self._open_bar = False
            GLib.idle_add(lambda: win._open_panel_dialog() and False)


def _screenshot(win, path):
    """Uso de desarrollo: guarda la ventana como PNG y sale."""
    w, h = win.get_width(), win.get_height()
    paintable = Gtk.WidgetPaintable.new(win)
    snap = Gtk.Snapshot()
    paintable.snapshot(snap, w, h)
    node = snap.to_node()
    if node:
        win.get_native().get_renderer().render_texture(node, None).save_to_png(path)
    win.get_application().quit()
    return False


if __name__ == '__main__':
    sys.exit(App().run([a for a in sys.argv if a != '--barra']))

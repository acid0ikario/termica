//! Ventana principal: lista de perfiles, editor y estado en vivo.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib::{self, clone};
use serde_json::Value;
use termica_core::{Cpu, Curves, FanMode, Fans, Gpu, Profile, ShiftMode, Store};

use crate::client::Client;
use crate::curve::CurveEditor;

const MEM_OPTIONS: &[(Option<u32>, &str)] =
    &[(None, "De fábrica (6001 MHz)"), (Some(5501), "5501 MHz"), (Some(810), "810 MHz")];
const FAN_OPTIONS: &[(FanMode, &str)] =
    &[(FanMode::Auto, "Fábrica (automático)"), (FanMode::Advanced, "Curva personalizada")];
const SHIFT_OPTIONS: &[(Option<ShiftMode>, &str)] = &[
    (None, "No cambiar"),
    (Some(ShiftMode::Eco), "Eco"),
    (Some(ShiftMode::Comfort), "Comfort"),
    (Some(ShiftMode::Turbo), "Turbo"),
];

fn index_of<T: PartialEq>(options: &[(T, &str)], value: &T) -> u32 {
    options.iter().position(|(v, _)| v == value).unwrap_or(0) as u32
}

fn labels<T>(options: &[(T, &str)]) -> gtk::StringList {
    gtk::StringList::new(&options.iter().map(|(_, l)| *l).collect::<Vec<_>>())
}

/// Tarjeta con un valor grande y un detalle.
struct Stat {
    root: gtk::Box,
    value: gtk::Label,
    detail: gtk::Label,
}

impl Stat {
    fn new(caption: &str) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .css_classes(["card", "stat"])
            .hexpand(true)
            .build();
        let value = gtk::Label::builder().label("—").xalign(0.0).css_classes(["stat-value"]).build();
        let detail = gtk::Label::builder().label(caption).xalign(0.0).css_classes(["dim-label", "caption"]).build();
        root.append(&value);
        root.append(&detail);
        Stat { root, value, detail }
    }

    fn set(&self, value: &str, detail: &str, temp: Option<i64>) {
        self.value.set_label(value);
        self.detail.set_label(detail);
        self.root.remove_css_class("stat-hot");
        self.root.remove_css_class("stat-burn");
        match temp {
            Some(t) if t >= 90 => self.root.add_css_class("stat-burn"),
            Some(t) if t >= 80 => self.root.add_css_class("stat-hot"),
            _ => {}
        }
    }
}

#[derive(Default)]
struct Model {
    data: Option<Store>,
    selected: Option<String>,
    curves: Option<Curves>,
    curve_key: String,
    status: Value,
}

pub struct Window {
    pub win: adw::ApplicationWindow,
    client: Client,
    model: RefCell<Model>,
    dirty: Cell<bool>,
    loading: Cell<bool>,
    poll: RefCell<Option<glib::SourceId>>,

    toasts: adw::ToastOverlay,
    stack: gtk::Stack,
    split: adw::NavigationSplitView,
    listbox: gtk::ListBox,
    st_cpu: Stat,
    st_gpu: Stat,
    st_fan: Stat,
    st_clock: Stat,
    content_page: adw::NavigationPage,
    save_btn: gtk::Button,
    hero_title: gtk::Label,
    hero_sub: gtk::Label,
    activate_btn: gtk::Button,
    name_row: adw::EntryRow,
    core_switch: adw::SwitchRow,
    core_spin: adw::SpinRow,
    mem_combo: adw::ComboRow,
    pl1: adw::SpinRow,
    pl2: adw::SpinRow,
    turbo: adw::SwitchRow,
    fan_combo: adw::ComboRow,
    boost: adw::SwitchRow,
    shift_combo: adw::ComboRow,
    curve_group: adw::PreferencesGroup,
    curve: CurveEditor,
    delete_row: adw::ButtonRow,
}

impl Window {
    pub fn new(app: &adw::Application, client: Client) -> Rc<Self> {
        let win = adw::ApplicationWindow::builder()
            .application(app)
            .title("Térmica")
            .default_width(1040)
            .default_height(760)
            .width_request(360)
            .height_request(480)
            .build();

        let toasts = adw::ToastOverlay::new();
        let split = adw::NavigationSplitView::builder().min_sidebar_width(250.0).max_sidebar_width(330.0).build();
        toasts.set_child(Some(&split));

        // ---------- barra lateral
        let side_header = adw::HeaderBar::new();
        let add_btn = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text("Nuevo perfil").build();
        side_header.pack_start(&add_btn);
        let bar_btn =
            gtk::Button::builder().icon_name("termica-symbolic").tooltip_text("Qué mostrar en la barra superior").build();
        side_header.pack_end(&bar_btn);

        let listbox = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let (st_cpu, st_gpu, st_fan, st_clock) =
            (Stat::new("Procesador"), Stat::new("Gráfica"), Stat::new("Ventiladores"), Stat::new("Núcleo GPU"));
        let grid = gtk::Grid::builder().column_spacing(8).row_spacing(8).column_homogeneous(true).build();
        grid.attach(&st_cpu.root, 0, 0, 1, 1);
        grid.attach(&st_gpu.root, 1, 0, 1, 1);
        grid.attach(&st_fan.root, 0, 1, 1, 1);
        grid.attach(&st_clock.root, 1, 1, 1, 1);
        let stats = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .margin_top(6)
            .build();
        stats.append(&gtk::Label::builder().label("Ahora mismo").xalign(0.0).css_classes(["heading"]).build());
        stats.append(&grid);
        let side_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side_box.append(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&listbox)
                .build(),
        );
        side_box.append(&stats);
        let side_tv = adw::ToolbarView::new();
        side_tv.add_top_bar(&side_header);
        side_tv.set_content(Some(&side_box));
        split.set_sidebar(Some(&adw::NavigationPage::new(&side_tv, "Térmica")));

        // ---------- contenido
        let header = adw::HeaderBar::new();
        let save_btn = gtk::Button::builder().label("Guardar").css_classes(["suggested-action"]).sensitive(false).build();
        header.pack_end(&save_btn);
        let page = adw::PreferencesPage::new();

        let hero = adw::PreferencesGroup::new();
        let hbox = gtk::Box::builder().spacing(12).margin_bottom(6).build();
        let tbox = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let hero_title = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["hero-title"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let hero_sub = gtk::Label::builder().xalign(0.0).css_classes(["dim-label"]).wrap(true).build();
        tbox.append(&hero_title);
        tbox.append(&hero_sub);
        let activate_btn = gtk::Button::builder()
            .label("Activar")
            .css_classes(["pill", "suggested-action"])
            .valign(gtk::Align::Center)
            .build();
        hbox.append(&tbox);
        hbox.append(&activate_btn);
        hero.add(&hbox);
        page.add(&hero);

        let g = adw::PreferencesGroup::builder().title("General").build();
        let name_row = adw::EntryRow::builder().title("Nombre del perfil").build();
        g.add(&name_row);
        page.add(&g);

        let g = adw::PreferencesGroup::builder()
            .title("Gráfica NVIDIA")
            .description(
                "RTX 3060 Laptop. De fábrica sube hasta 2100 MHz. Limitarla baja el calor y el ruido, \
                 pero también la velocidad en juegos y renders de Blender.",
            )
            .build();
        let core_switch = adw::SwitchRow::builder().title("Limitar velocidad del núcleo").build();
        let core_spin = adw::SpinRow::with_range(300.0, 2100.0, 50.0);
        core_spin.set_title("Velocidad máxima del núcleo");
        core_spin.set_subtitle("MHz");
        core_switch.bind_property("active", &core_spin, "sensitive").sync_create().build();
        let mem_combo = adw::ComboRow::builder()
            .title("Memoria máxima")
            .subtitle("Bajarla casi no reduce el calor")
            .model(&labels(MEM_OPTIONS))
            .build();
        g.add(&core_switch);
        g.add(&core_spin);
        g.add(&mem_combo);
        page.add(&g);

        let g = adw::PreferencesGroup::builder()
            .title("Procesador")
            .description("Intel i7-1260P. De fábrica: 80 W sostenido y 200 W en ráfagas.")
            .build();
        let pl1 = adw::SpinRow::with_range(5.0, 115.0, 1.0);
        pl1.set_title("Potencia sostenida (PL1)");
        pl1.set_subtitle("Vatios que puede usar todo el tiempo");
        let pl2 = adw::SpinRow::with_range(5.0, 200.0, 1.0);
        pl2.set_title("Potencia de ráfaga (PL2)");
        pl2.set_subtitle("Vatios por unos segundos al arrancar una tarea");
        let turbo = adw::SwitchRow::builder()
            .title("Turbo Boost")
            .subtitle("Apagarlo baja mucho el calor; el CPU queda en su velocidad base")
            .build();
        g.add(&pl1);
        g.add(&pl2);
        g.add(&turbo);
        page.add(&g);

        let g = adw::PreferencesGroup::builder().title("Ventiladores").build();
        let fan_combo = adw::ComboRow::builder().title("Control").model(&labels(FAN_OPTIONS)).build();
        let boost =
            adw::SwitchRow::builder().title("Cooler Boost").subtitle("Ventiladores al máximo todo el tiempo").build();
        let shift_combo = adw::ComboRow::builder()
            .title("Modo del firmware MSI")
            .subtitle("Ajuste interno del portátil (Eco, Comfort o Turbo)")
            .model(&labels(SHIFT_OPTIONS))
            .build();
        g.add(&fan_combo);
        g.add(&boost);
        g.add(&shift_combo);
        page.add(&g);

        let curve_group = adw::PreferencesGroup::builder()
            .title("Curva de ventilador")
            .description(
                "Arrastra los puntos: a la derecha cambia la temperatura y hacia arriba la velocidad. \
                 La línea punteada es la temperatura actual.",
            )
            .build();
        let toggles = adw::ToggleGroup::builder().valign(gtk::Align::Center).build();
        toggles.add(adw::Toggle::builder().name("cpu").label("CPU").build());
        toggles.add(adw::Toggle::builder().name("gpu").label("GPU").build());
        toggles.set_active_name(Some("cpu"));
        curve_group.set_header_suffix(Some(&toggles));
        let curve = CurveEditor::new();
        let frame = gtk::Box::builder().css_classes(["card", "curve-card"]).build();
        frame.append(&curve.area);
        curve_group.add(&frame);
        page.add(&curve_group);

        let g = adw::PreferencesGroup::new();
        let dup_row = adw::ButtonRow::builder().title("Duplicar perfil").start_icon_name("edit-copy-symbolic").build();
        let delete_row = adw::ButtonRow::builder()
            .title("Eliminar perfil")
            .start_icon_name("user-trash-symbolic")
            .css_classes(["destructive-action"])
            .build();
        g.add(&dup_row);
        g.add(&delete_row);
        page.add(&g);

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&page));
        let content_page = adw::NavigationPage::new(&tv, "Perfil");
        split.set_content(Some(&content_page));

        // ---------- servicio apagado
        let unavailable = adw::ToolbarView::new();
        unavailable.add_top_bar(&adw::HeaderBar::new());
        unavailable.set_content(Some(
            &adw::StatusPage::builder()
                .icon_name("dialog-warning-symbolic")
                .title("El servicio no está corriendo")
                .description("Térmica necesita el servicio termicad.\nEjecútalo con: sudo systemctl start termicad")
                .build(),
        ));

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        stack.add_named(&toasts, Some("main"));
        stack.add_named(&unavailable, Some("unavailable"));
        win.set_content(Some(&stack));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 680sp").unwrap());
        bp.add_setter(&split, "collapsed", Some(&true.to_value()));
        win.add_breakpoint(bp);

        let w = Rc::new(Window {
            win,
            client,
            model: RefCell::new(Model { curve_key: "cpu".into(), ..Default::default() }),
            dirty: Cell::new(false),
            loading: Cell::new(false),
            poll: RefCell::new(None),
            toasts,
            stack,
            split,
            listbox,
            st_cpu,
            st_gpu,
            st_fan,
            st_clock,
            content_page,
            save_btn,
            hero_title,
            hero_sub,
            activate_btn,
            name_row,
            core_switch,
            core_spin,
            mem_combo,
            pl1,
            pl2,
            turbo,
            fan_combo,
            boost,
            shift_combo,
            curve_group,
            curve,
            delete_row,
        });

        // ---------- señales
        let changed = clone!(#[weak] w, move || w.changed());
        w.name_row.connect_changed(clone!(#[strong] changed, move |_| changed()));
        for row in [&w.core_switch, &w.turbo, &w.boost] {
            row.connect_active_notify(clone!(#[strong] changed, move |_| changed()));
        }
        for row in [&w.core_spin, &w.pl1, &w.pl2] {
            row.connect_value_notify(clone!(#[strong] changed, move |_| changed()));
        }
        for row in [&w.mem_combo, &w.fan_combo, &w.shift_combo] {
            row.connect_selected_notify(clone!(#[strong] changed, move |_| changed()));
        }
        w.curve.connect_changed(clone!(#[weak] w, move || {
            let key = w.model.borrow().curve_key.clone();
            let c = w.curve.curve();
            if let Some(curves) = w.model.borrow_mut().curves.as_mut() {
                if key == "gpu" { curves.gpu = c } else { curves.cpu = c }
            }
            w.changed();
        }));
        toggles.connect_active_name_notify(clone!(#[weak] w, move |t| {
            let key = t.active_name().map(|s| s.to_string()).unwrap_or_else(|| "cpu".into());
            w.model.borrow_mut().curve_key = key.clone();
            if let Some(curves) = w.model.borrow().curves.as_ref() {
                w.curve.set_curve(if key == "gpu" { &curves.gpu } else { &curves.cpu });
            }
            w.update_curve_current();
        }));
        w.listbox.connect_row_activated(clone!(#[weak] w, move |_, row| w.row_activated(row.widget_name().as_str())));
        w.save_btn.connect_clicked(clone!(#[weak] w, move |_| w.save(false)));
        w.activate_btn.connect_clicked(clone!(#[weak] w, move |_| w.activate()));
        add_btn.connect_clicked(clone!(#[weak] w, move |_| w.new_profile(false)));
        dup_row.connect_activated(clone!(#[weak] w, move |_| w.new_profile(true)));
        w.delete_row.connect_activated(clone!(#[weak] w, move |_| w.confirm_delete()));
        bar_btn.connect_clicked(clone!(#[weak] w, move |_| w.open_panel_dialog()));

        w.client.connect_signal(clone!(#[weak] w, move |name| {
            if name == "ActiveChanged" || name == "ProfilesChanged" {
                w.reload(None);
            }
        }));
        w.client.connect_available(clone!(#[weak] w, move |ok| w.on_available(ok)));
        w.on_available(w.client.available());

        let id = glib::timeout_add_local(Duration::from_secs(2), clone!(
            #[weak] w,
            #[upgrade_or] glib::ControlFlow::Break,
            move || {
                w.poll_status();
                glib::ControlFlow::Continue
            }
        ));
        *w.poll.borrow_mut() = Some(id);
        w.win.connect_close_request(clone!(#[weak] w, #[upgrade_or] glib::Propagation::Proceed, move |_| {
            if let Some(id) = w.poll.borrow_mut().take() {
                id.remove();
            }
            glib::Propagation::Proceed
        }));
        w
    }

    pub fn open_panel_dialog(&self) {
        if !crate::panel::present(&self.win) {
            self.toast("La extensión de la barra no está instalada");
        }
    }

    fn toast(&self, text: &str) {
        let toast = adw::Toast::new(&glib::markup_escape_text(text));
        toast.set_timeout(4);
        self.toasts.add_toast(toast);
    }

    fn on_available(self: &Rc<Self>, ok: bool) {
        self.stack.set_visible_child_name(if ok { "main" } else { "unavailable" });
        if ok {
            self.reload(None);
            self.poll_status();
        }
    }

    // ------------------------------------------------------------------ datos

    fn reload(self: &Rc<Self>, select: Option<String>) {
        let w = self.clone();
        glib::spawn_future_local(async move {
            let store = match w.client.call("GetProfiles", None).await {
                Ok(Some(json)) => match serde_json::from_str::<Store>(&json) {
                    Ok(s) => s,
                    Err(e) => return w.toast(&format!("Respuesta inválida del servicio: {e}")),
                },
                Ok(None) => return,
                Err(e) => return w.toast(&e),
            };
            let forced = select.is_some();
            let target = select
                .or_else(|| w.model.borrow().selected.clone())
                .filter(|id| store.get(id).is_some())
                .unwrap_or_else(|| store.active.clone());
            w.model.borrow_mut().data = Some(store);
            w.fill_list();
            if !w.dirty.get() || forced {
                w.show(&target);
            } else {
                w.update_hero();
            }
        });
    }

    fn fill_list(&self) {
        self.listbox.remove_all();
        let m = self.model.borrow();
        let Some(store) = m.data.as_ref() else { return };
        for p in &store.profiles {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&p.name))
                .subtitle(p.summary())
                .activatable(true)
                .subtitle_lines(2)
                .name(&p.id)
                .build();
            let icon = gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .visible(p.id == store.active)
                .tooltip_text("Perfil activo")
                .build();
            row.add_suffix(&icon);
            self.listbox.append(&row);
            if Some(&p.id) == m.selected.as_ref() {
                self.listbox.select_row(Some(&row));
            }
        }
    }

    fn select_row(&self, id: &str) {
        let mut child = self.listbox.first_child();
        while let Some(c) = child {
            if c.widget_name() == id
                && let Ok(row) = c.clone().downcast::<gtk::ListBoxRow>()
            {
                self.listbox.select_row(Some(&row));
            }
            child = c.next_sibling();
        }
    }

    fn show(&self, id: &str) {
        let Some(p) = self.model.borrow().data.as_ref().and_then(|s| s.get(id).cloned()) else { return };
        self.loading.set(true);
        self.name_row.set_text(&p.name);
        self.core_switch.set_active(p.gpu.core_max.is_some());
        self.core_spin.set_value(p.gpu.core_max.unwrap_or(1400) as f64);
        self.mem_combo.set_selected(index_of(MEM_OPTIONS, &p.gpu.mem_max));
        self.pl1.set_value(p.cpu.pl1 as f64);
        self.pl2.set_value(p.cpu.pl2 as f64);
        self.turbo.set_active(p.cpu.turbo);
        self.fan_combo.set_selected(index_of(FAN_OPTIONS, &p.fans.mode));
        self.boost.set_active(p.fans.cooler_boost);
        self.shift_combo.set_selected(index_of(SHIFT_OPTIONS, &p.shift_mode));
        {
            let mut m = self.model.borrow_mut();
            m.selected = Some(id.to_string());
            m.curves = Some(p.fans.curves.clone());
            let c = if m.curve_key == "gpu" { &p.fans.curves.gpu } else { &p.fans.curves.cpu };
            self.curve.set_curve(c);
        }
        self.loading.set(false);
        self.set_dirty(false);
        self.select_row(id);
    }

    fn update_hero(&self) {
        let m = self.model.borrow();
        let (Some(store), Some(sel)) = (m.data.as_ref(), m.selected.as_ref()) else { return };
        let active = &store.active == sel;
        let name = self.name_row.text();
        let name = if name.is_empty() { "Sin nombre".into() } else { name.to_string() };
        self.hero_title.set_label(&name);
        self.content_page.set_title(&name);
        self.hero_sub.set_label(if active { "Este es el perfil activo" } else { "Perfil guardado, no está activo" });
        if self.dirty.get() {
            self.activate_btn.set_label(if active { "Guardar y aplicar" } else { "Guardar y activar" });
            self.activate_btn.set_sensitive(true);
        } else {
            self.activate_btn.set_label(if active { "Activo" } else { "Activar" });
            self.activate_btn.set_sensitive(!active);
        }
        self.delete_row.set_sensitive(!active && store.profiles.len() > 1);
        self.curve_group.set_visible(FAN_OPTIONS[self.fan_combo.selected() as usize].0 == FanMode::Advanced);
    }

    fn collect(&self) -> Profile {
        let m = self.model.borrow();
        Profile {
            id: m.selected.clone().unwrap_or_default(),
            name: self.name_row.text().trim().to_string(),
            gpu: Gpu {
                core_max: self.core_switch.is_active().then(|| self.core_spin.value() as u32),
                mem_max: MEM_OPTIONS[self.mem_combo.selected() as usize].0,
            },
            cpu: Cpu { pl1: self.pl1.value() as u32, pl2: self.pl2.value() as u32, turbo: self.turbo.is_active() },
            fans: Fans {
                mode: FAN_OPTIONS[self.fan_combo.selected() as usize].0,
                cooler_boost: self.boost.is_active(),
                curves: m.curves.clone().unwrap_or_default(),
            },
            shift_mode: SHIFT_OPTIONS[self.shift_combo.selected() as usize].0,
        }
    }

    // ------------------------------------------------------------------ eventos

    fn changed(&self) {
        if self.loading.get() {
            return;
        }
        if self.pl2.value() < self.pl1.value() {
            self.pl2.set_value(self.pl1.value());
        }
        self.set_dirty(true);
    }

    fn set_dirty(&self, dirty: bool) {
        self.dirty.set(dirty);
        self.save_btn.set_sensitive(dirty);
        self.update_hero();
    }

    fn row_activated(self: &Rc<Self>, id: &str) {
        let id = id.to_string();
        if self.model.borrow().selected.as_deref() == Some(id.as_str()) {
            self.split.set_show_content(true);
            return;
        }
        if !self.dirty.get() {
            self.show(&id);
            self.split.set_show_content(true);
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some("¿Descartar cambios?"),
            Some(&format!("Tienes cambios sin guardar en «{}».", self.name_row.text())),
        );
        dialog.add_response("cancel", "Cancelar");
        dialog.add_response("discard", "Descartar");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        let w = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&w.win)).await == "discard" {
                w.set_dirty(false);
                w.show(&id);
                w.split.set_show_content(true);
            } else if let Some(sel) = w.model.borrow().selected.clone() {
                w.select_row(&sel);
            }
        });
    }

    fn update_curve_current(&self) {
        let m = self.model.borrow();
        let key = if m.curve_key == "gpu" { "gpu_temp" } else { "cpu_temp" };
        self.curve.set_current(m.status.get(key).and_then(Value::as_i64));
    }

    // ------------------------------------------------------------------ acciones

    /// Guarda; si `then_activate`, activa el perfil guardado.
    fn save(self: &Rc<Self>, then_activate: bool) {
        let json = serde_json::to_string(&self.collect()).unwrap();
        let w = self.clone();
        glib::spawn_future_local(async move {
            let out = match w.client.call("SaveProfile", Some(&json)).await {
                Ok(Some(out)) => out,
                Ok(None) => return,
                Err(e) => return w.toast(&e),
            };
            let out: Value = serde_json::from_str(&out).unwrap_or_default();
            let id = out["id"].as_str().unwrap_or_default().to_string();
            let warnings: Vec<&str> =
                out["warnings"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            w.set_dirty(false);
            w.toast(&if warnings.is_empty() { "Guardado".into() } else { format!("Guardado: {}", warnings.join("; ")) });
            w.reload(Some(id.clone()));
            if then_activate {
                w.activate_id(id).await;
            }
        });
    }

    fn activate(self: &Rc<Self>) {
        if self.dirty.get() {
            self.save(true);
        } else if let Some(id) = self.model.borrow().selected.clone() {
            let w = self.clone();
            glib::spawn_future_local(async move { w.activate_id(id).await });
        }
    }

    async fn activate_id(&self, id: String) {
        match self.client.call("Activate", Some(&id)).await {
            Err(e) => self.toast(&e),
            Ok(warnings) => {
                let name = self
                    .model
                    .borrow()
                    .data
                    .as_ref()
                    .and_then(|s| s.get(&id).map(|p| p.name.clone()))
                    .unwrap_or(id);
                let warnings = warnings.unwrap_or_default();
                self.toast(&if warnings.is_empty() {
                    format!("Perfil «{name}» activado")
                } else {
                    format!("Perfil «{name}» activado. Avisos: {warnings}")
                });
            }
        }
    }

    fn new_profile(self: &Rc<Self>, duplicate: bool) {
        let mut base = {
            let m = self.model.borrow();
            let Some(store) = m.data.as_ref() else { return };
            if duplicate && m.selected.is_some() {
                drop(m);
                self.collect()
            } else {
                store.active_profile().clone()
            }
        };
        base.id = String::new();
        base.name = if duplicate { format!("{} (copia)", base.name).chars().take(40).collect() } else { "Nuevo perfil".into() };
        let json = serde_json::to_string(&base).unwrap();
        let w = self.clone();
        glib::spawn_future_local(async move {
            match w.client.call("SaveProfile", Some(&json)).await {
                Err(e) => w.toast(&e),
                Ok(out) => {
                    let out: Value = serde_json::from_str(&out.unwrap_or_default()).unwrap_or_default();
                    w.dirty.set(false);
                    w.reload(out["id"].as_str().map(str::to_string));
                    w.split.set_show_content(true);
                    w.toast("Perfil creado");
                }
            }
        });
    }

    fn confirm_delete(self: &Rc<Self>) {
        let name = self.name_row.text().to_string();
        let dialog = adw::AlertDialog::new(Some(&format!("¿Eliminar «{name}»?")), Some("Esta acción no se puede deshacer."));
        dialog.add_response("cancel", "Cancelar");
        dialog.add_response("delete", "Eliminar");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let w = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&w.win)).await != "delete" {
                return;
            }
            let Some(id) = w.model.borrow().selected.clone() else { return };
            match w.client.call("DeleteProfile", Some(&id)).await {
                Err(e) => w.toast(&e),
                Ok(_) => {
                    w.model.borrow_mut().selected = None;
                    w.dirty.set(false);
                    let active = w.model.borrow().data.as_ref().map(|s| s.active.clone());
                    w.reload(active);
                    w.toast(&format!("«{name}» eliminado"));
                }
            }
        });
    }

    // ------------------------------------------------------------------ estado en vivo

    fn poll_status(self: &Rc<Self>) {
        if !self.client.available() {
            return;
        }
        let w = self.clone();
        glib::spawn_future_local(async move {
            let Ok(Some(json)) = w.client.call("GetStatus", None).await else { return };
            let s: Value = serde_json::from_str(&json).unwrap_or_default();
            let int = |k: &str| s.get(k).and_then(Value::as_i64);
            let cpu_t = int("cpu_temp");
            w.st_cpu.set(
                &cpu_t.map(|t| format!("{t}°")).unwrap_or("—".into()),
                &format!("Procesador · {}/{} W", int("pl1").unwrap_or(0), int("pl2").unwrap_or(0)),
                cpu_t,
            );
            if s["gpu_awake"].as_bool() == Some(true) {
                let gpu_t = int("gpu_temp");
                let pw = int("gpu_power").map(|p| format!(" · {p} W")).unwrap_or_default();
                w.st_gpu.set(&gpu_t.map(|t| format!("{t}°")).unwrap_or("—".into()), &format!("Gráfica{pw}"), gpu_t);
                w.st_clock.set(
                    &int("gpu_clock").map(|c| c.to_string()).unwrap_or("—".into()),
                    &format!("MHz núcleo · {}", s["gpu_pstate"].as_str().unwrap_or("")),
                    None,
                );
            } else {
                w.st_gpu.set("Dormida", "Gráfica sin uso", None);
                w.st_clock.set("0", "MHz núcleo", None);
            }
            let mode = match s["fan_mode"].as_str() {
                Some("auto") => "auto",
                Some("advanced") => "curva",
                Some(other) => other,
                None => "",
            };
            let boost = if s["cooler_boost"].as_bool() == Some(true) { " · boost" } else { "" };
            w.st_fan.set(
                &format!("{}%", int("cpu_fan").unwrap_or(0)),
                &format!("CPU · GPU {}% · {mode}{boost}", int("gpu_fan").unwrap_or(0)),
                None,
            );
            w.model.borrow_mut().status = s;
            w.update_curve_current();
        });
    }
}

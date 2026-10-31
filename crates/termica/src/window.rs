//! Ventana principal: lista de perfiles, editor y estado en vivo.

use crate::i18n::tr;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};
use serde_json::Value;
use termica_core::{
    BOOST_SECS_RANGE, BOOST_TEMP_RANGE, Cpu, Curves, FanMode, Fans, Gpu, Profile, ShiftMode, Store, default_profile,
};

use crate::client::Client;
use crate::curve::CurveEditor;

const MEM_OPTIONS: &[(Option<u32>, &str)] =
    &[(None, "Factory default (6001 MHz)"), (Some(5501), "5501 MHz"), (Some(810), "810 MHz")];
const FAN_OPTIONS: &[(FanMode, &str)] =
    &[(FanMode::Auto, "Factory default (automatic)"), (FanMode::Advanced, "Custom curve")];
const SHIFT_OPTIONS: &[(Option<ShiftMode>, &str)] = &[
    (None, "Leave unchanged"),
    (Some(ShiftMode::Eco), "Eco"),
    (Some(ShiftMode::Comfort), "Comfort"),
    (Some(ShiftMode::Turbo), "Turbo"),
];

/// Qué hace cada modo del firmware (medido en el Stealth 15M; «performance» lo anuncia el
/// firmware pero msi-ec lo rechaza, por eso no aparece).
fn shift_hint(mode: Option<ShiftMode>) -> &'static str {
    match mode {
        None => tr("Térmica leaves the laptop's current firmware mode unchanged"),
        Some(ShiftMode::Eco) => tr("Most restricted: lowest fan speed cap, for quiet operation and battery life"),
        Some(ShiftMode::Comfort) => tr("Caps fans at about 4200 rpm, even if the curve requests more"),
        Some(ShiftMode::Turbo) => tr("No cap: fans can reach the speed requested by the curve"),
    }
}

fn index_of<T: PartialEq>(options: &[(T, &str)], value: &T) -> u32 {
    options.iter().position(|(v, _)| v == value).unwrap_or(0) as u32
}

fn labels<T>(options: &[(T, &str)]) -> gtk::StringList {
    gtk::StringList::new(&options.iter().map(|(_, l)| tr(l)).collect::<Vec<_>>())
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
    auto_boost: adw::SwitchRow,
    boost_at: adw::SpinRow,
    boost_secs: adw::SpinRow,
    shift_combo: adw::ComboRow,
    curve_group: adw::PreferencesGroup,
    curve: CurveEditor,
    delete_row: adw::ButtonRow,
    restore_row: adw::ButtonRow,
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

        // ---------- barra lateral
        let side_header = adw::HeaderBar::new();
        let add_btn = gtk::Button::builder().icon_name("list-add-symbolic").tooltip_text(tr("New profile")).build();
        side_header.pack_start(&add_btn);
        let bar_btn =
            gtk::Button::builder().icon_name("termica-symbolic").tooltip_text(tr("Choose what appears in the top bar")).build();
        let menu = gio::Menu::new();
        menu.append(Some(tr("Restore the 3 default profiles")), Some("win.restore-defaults"));
        let languages = gio::Menu::new();
        for (code, label) in [("system", "System"), ("en", "English"), ("es", "Spanish")] {
            languages.append(Some(tr(label)), Some(&format!("win.language::{}", code)));
        }
        menu.append_submenu(Some(tr("Language")), &languages);
        menu.append(Some(tr("Restart Térmica")), Some("win.restart"));
        let menu_btn = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text(tr("Menu"))
            .menu_model(&menu)
            .build();
        side_header.pack_end(&menu_btn);
        side_header.pack_end(&bar_btn);

        let listbox = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let (st_cpu, st_gpu, st_fan, st_clock) =
            (Stat::new(tr("Processor")), Stat::new(tr("Graphics")), Stat::new(tr("Fans")), Stat::new(tr("GPU core")));
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
        stats.append(&gtk::Label::builder().label(tr("Right now")).xalign(0.0).css_classes(["heading"]).build());
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
        let save_btn = gtk::Button::builder().label(tr("Save")).css_classes(["suggested-action"]).sensitive(false).build();
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
            .label(tr("Activate"))
            .css_classes(["pill", "suggested-action"])
            .valign(gtk::Align::Center)
            .build();
        hbox.append(&tbox);
        hbox.append(&activate_btn);
        hero.add(&hbox);
        page.add(&hero);

        let g = adw::PreferencesGroup::builder().title("General").build();
        let name_row = adw::EntryRow::builder().title(tr("Profile name")).build();
        g.add(&name_row);
        page.add(&g);

        let g = adw::PreferencesGroup::builder()
            .title(tr("NVIDIA graphics"))
            .description(
                tr("RTX 3060 Laptop. Factory clocks reach 2100 MHz. Limiting them reduces heat and noise, but also performance in games and Blender renders."),
            )
            .build();
        let core_switch = adw::SwitchRow::builder().title(tr("Limit core clock")).build();
        let core_spin = adw::SpinRow::with_range(300.0, 2100.0, 50.0);
        core_spin.set_title(tr("Maximum core clock"));
        core_spin.set_subtitle("MHz");
        core_switch.bind_property("active", &core_spin, "sensitive").sync_create().build();
        let mem_combo = adw::ComboRow::builder()
            .title(tr("Maximum memory clock"))
            .subtitle(tr("Lowering it barely reduces heat"))
            .model(&labels(MEM_OPTIONS))
            .build();
        g.add(&core_switch);
        g.add(&core_spin);
        g.add(&mem_combo);
        page.add(&g);

        let g = adw::PreferencesGroup::builder()
            .title(tr("Processor"))
            .description(tr("Intel i7-1260P. Factory defaults: 80 W sustained and 200 W burst."))
            .build();
        let pl1 = adw::SpinRow::with_range(5.0, 115.0, 1.0);
        pl1.set_title(tr("Sustained power (PL1)"));
        pl1.set_subtitle(tr("Watts available continuously"));
        let pl2 = adw::SpinRow::with_range(5.0, 200.0, 1.0);
        pl2.set_title(tr("Burst power (PL2)"));
        pl2.set_subtitle(tr("Watts available briefly when starting a task"));
        let turbo = adw::SwitchRow::builder()
            .title("Turbo Boost")
            .subtitle(tr("Turning it off greatly reduces heat; the CPU runs at its base clock"))
            .build();
        g.add(&pl1);
        g.add(&pl2);
        g.add(&turbo);
        page.add(&g);

        let g = adw::PreferencesGroup::builder().title(tr("Fans")).build();
        let fan_combo = adw::ComboRow::builder().title("Control").model(&labels(FAN_OPTIONS)).build();
        let boost =
            adw::SwitchRow::builder().title("Cooler Boost").subtitle(tr("Run fans at maximum speed continuously")).build();
        let shift_combo = adw::ComboRow::builder()
            .title(tr("MSI firmware mode"))
            .subtitle(shift_hint(None))
            .subtitle_lines(2)
            .model(&labels(SHIFT_OPTIONS))
            .build();
        shift_combo.connect_selected_notify(|row| {
            row.set_subtitle(shift_hint(SHIFT_OPTIONS[row.selected() as usize].0));
        });
        let auto_boost = adw::SwitchRow::builder()
            .title(tr("Automatic Cooler Boost"))
            .subtitle(tr("Turns it on briefly when the CPU or GPU gets hot to keep temperatures under control"))
            .build();
        let boost_at = adw::SpinRow::with_range(BOOST_TEMP_RANGE.0 as f64, BOOST_TEMP_RANGE.1 as f64, 1.0);
        boost_at.set_title(tr("Turn on at"));
        boost_at.set_subtitle(tr("°C, on the CPU or GPU"));
        let boost_secs = adw::SpinRow::with_range(BOOST_SECS_RANGE.0 as f64, BOOST_SECS_RANGE.1 as f64, 5.0);
        boost_secs.set_title(tr("Keep on for"));
        boost_secs.set_subtitle(tr("Seconds after cooling below that temperature"));
        g.add(&fan_combo);
        g.add(&boost);
        g.add(&auto_boost);
        g.add(&boost_at);
        g.add(&boost_secs);
        g.add(&shift_combo);
        page.add(&g);

        let curve_group = adw::PreferencesGroup::builder()
            .title(tr("Fan curve"))
            .description(
                tr("Drag points: right changes temperature and up changes speed. The dotted line shows the current temperature."),
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
        let restore_row = adw::ButtonRow::builder()
            .title(tr("Restore defaults"))
            .start_icon_name("edit-undo-symbolic")
            .build();
        g.add(&restore_row);
        let dup_row = adw::ButtonRow::builder().title(tr("Duplicate profile")).start_icon_name("edit-copy-symbolic").build();
        let delete_row = adw::ButtonRow::builder()
            .title(tr("Delete profile"))
            .start_icon_name("user-trash-symbolic")
            .css_classes(["destructive-action"])
            .build();
        g.add(&dup_row);
        g.add(&delete_row);
        page.add(&g);

        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&header);
        tv.set_content(Some(&page));
        let content_page = adw::NavigationPage::new(&tv, tr("Profile"));
        split.set_content(Some(&content_page));

        // ---------- servicio apagado
        let unavailable = adw::ToolbarView::new();
        let unavailable_header = adw::HeaderBar::new();
        let language_menu = gio::Menu::new();
        language_menu.append_submenu(Some(tr("Language")), &languages);
        language_menu.append(Some(tr("Restart Térmica")), Some("win.restart"));
        unavailable_header.pack_end(&gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic").tooltip_text(tr("Menu"))
            .menu_model(&language_menu).build());
        unavailable.add_top_bar(&unavailable_header);
        unavailable.set_content(Some(
            &adw::StatusPage::builder()
                .icon_name("dialog-warning-symbolic")
                .title(tr("Service unavailable"))
                .description(tr("Térmica needs the termicad service.\nStart it with: sudo systemctl start termicad"))
                .build(),
        ));

        let stack = gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build();
        stack.add_named(&split, Some("main"));
        stack.add_named(&unavailable, Some("unavailable"));
        toasts.set_child(Some(&stack));
        win.set_content(Some(&toasts));

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
            auto_boost,
            boost_at,
            boost_secs,
            shift_combo,
            curve_group,
            curve,
            delete_row,
            restore_row,
        });

        // ---------- señales
        let changed = clone!(#[weak] w, move || w.changed());
        w.name_row.connect_changed(clone!(#[strong] changed, move |_| changed()));
        for row in [&w.core_switch, &w.turbo, &w.boost, &w.auto_boost] {
            row.connect_active_notify(clone!(#[strong] changed, move |_| changed()));
        }
        for row in [&w.core_spin, &w.pl1, &w.pl2, &w.boost_at, &w.boost_secs] {
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
        w.restore_row.connect_activated(clone!(#[weak] w, move |_| {
            let id = w.model.borrow().selected.clone();
            w.confirm_restore(id);
        }));
        let restore_all = gio::SimpleAction::new("restore-defaults", None);
        restore_all.connect_activate(clone!(#[weak] w, move |_, _| w.confirm_restore(None)));
        w.win.add_action(&restore_all);
        let language = gio::SimpleAction::new_stateful(
            "language", Some(glib::VariantTy::STRING), &crate::i18n::language().to_variant(),
        );
        language.connect_activate(clone!(#[weak] w, move |action, value| {
            let Some(value) = value.and_then(|v| v.str()) else { return };
            if !matches!(value, "system" | "en" | "es") { return; }
            match crate::i18n::set_language(value) {
                Ok(()) => {
                    action.set_state(&value.to_variant());
                    w.toast(tr("Restart Térmica to apply the language."));
                }
                Err(_) => w.toast(tr("Could not save language preference")),
            }
        }));
        w.win.add_action(&language);
        let restart = gio::SimpleAction::new("restart", None);
        restart.connect_activate(clone!(#[weak] w, move |_, _| w.confirm_restart()));
        w.win.add_action(&restart);
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
            self.toast(tr("The top bar extension is not installed"));
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
                    Err(e) => return w.toast(&trf!("Invalid service response: {e}", "Respuesta inválida del servicio: {e}")),
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
                .subtitle(profile_summary(p))
                .activatable(true)
                .subtitle_lines(2)
                .name(&p.id)
                .build();
            let icon = gtk::Image::builder()
                .icon_name("object-select-symbolic")
                .visible(p.id == store.active)
                .tooltip_text(tr("Active profile"))
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
        self.auto_boost.set_active(p.fans.boost_at.is_some());
        self.boost_at.set_value(p.fans.boost_at.unwrap_or(83) as f64);
        self.boost_secs.set_value(p.fans.boost_secs as f64);
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
        let name = if name.is_empty() { tr("Unnamed").into() } else { name.to_string() };
        self.hero_title.set_label(&name);
        self.content_page.set_title(&name);
        self.hero_sub.set_label(if active { tr("This is the active profile") } else { tr("Saved profile, currently inactive") });
        if self.dirty.get() {
            self.activate_btn.set_label(if active { tr("Save and apply") } else { tr("Save and activate") });
            self.activate_btn.set_sensitive(true);
        } else {
            self.activate_btn.set_label(if active { tr("Active") } else { tr("Activate") });
            self.activate_btn.set_sensitive(!active);
        }
        self.delete_row.set_sensitive(!active && store.profiles.len() > 1);
        let default = default_profile(sel);
        self.restore_row.set_visible(default.is_some());
        drop(m);
        if let Some(d) = default {
            let differs = self.collect() != d;
            self.restore_row.set_sensitive(differs);
            self.restore_row.set_tooltip_text(Some(if differs {
                tr("Restore this profile's original settings")
            } else {
                tr("Already using default settings")
            }));
        }
        // con el Cooler Boost siempre encendido, el automático no tiene sentido
        self.auto_boost.set_sensitive(!self.boost.is_active());
        let auto = self.auto_boost.is_active() && !self.boost.is_active();
        self.boost_at.set_visible(auto);
        self.boost_secs.set_visible(auto);
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
                boost_at: self.auto_boost.is_active().then(|| self.boost_at.value() as u32),
                boost_secs: self.boost_secs.value() as u32,
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
            Some(tr("Discard changes?")),
            Some(&trf!("You have unsaved changes in “{}”.", "Tienes cambios sin guardar en «{}».", self.name_row.text())),
        );
        dialog.add_response("cancel", tr("Cancel"));
        dialog.add_response("discard", tr("Discard"));
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

    fn confirm_restart(self: &Rc<Self>) {
        let Some(app) = self.win.application() else { return };
        if !self.dirty.get() {
            app.activate_action("restart", None);
            return;
        }
        let dialog = adw::AlertDialog::new(
            Some(tr("Restart Térmica?")),
            Some(tr("You have unsaved changes. Restarting will discard them.")),
        );
        dialog.add_response("cancel", tr("Cancel"));
        dialog.add_response("restart", tr("Discard and restart"));
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("cancel"));
        dialog.set_response_appearance("restart", adw::ResponseAppearance::Destructive);
        let w = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&w.win)).await == "restart" {
                app.activate_action("restart", None);
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
            w.toast(&if warnings.is_empty() { tr("Saved").into() } else { trf!("Saved: {}", "Guardado: {}", warnings.join("; ")) });
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
                    trf!("Profile “{name}” activated", "Perfil «{name}» activado")
                } else {
                    trf!("Profile “{name}” activated. Warnings: {warnings}", "Perfil «{name}» activado. Avisos: {warnings}")
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
        base.name = if duplicate { trf!("{} (copy)", "{} (copia)", base.name).chars().take(40).collect() } else { tr("New profile").into() };
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
                    w.toast(tr("Profile created"));
                }
            }
        });
    }

    fn confirm_delete(self: &Rc<Self>) {
        let name = self.name_row.text().to_string();
        let dialog = adw::AlertDialog::new(Some(&trf!("Delete “{name}”?", "¿Eliminar «{name}»?")), Some(tr("This action cannot be undone.")));
        dialog.add_response("cancel", tr("Cancel"));
        dialog.add_response("delete", tr("Delete"));
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
                    w.toast(&trf!("“{name}” deleted", "«{name}» eliminado"));
                }
            }
        });
    }

    /// Restaura un perfil incluido (o los tres con `None`) tras confirmar.
    fn confirm_restore(self: &Rc<Self>, id: Option<String>) {
        let (heading, body) = match &id {
            Some(_) => (
                trf!("Restore “{}”?", "¿Restaurar «{}»?", self.name_row.text()),
                tr("Restore the default settings. Your changes to this profile will be lost.").to_string(),
            ),
            None => (
                tr("Restore default profiles?").to_string(),
                tr("Quiet, Balanced and Factory return to their defaults (and reappear if deleted). Your custom profiles remain unchanged.")
                    .to_string(),
            ),
        };
        let dialog = adw::AlertDialog::new(Some(&heading), Some(&body));
        dialog.add_response("cancel", tr("Cancel"));
        dialog.add_response("restore", tr("Restore"));
        dialog.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
        let w = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&w.win)).await != "restore" {
                return;
            }
            match w.client.call("RestoreDefaults", Some(id.as_deref().unwrap_or(""))).await {
                Err(e) => w.toast(&e),
                Ok(warnings) => {
                    w.dirty.set(false);
                    let select = w.model.borrow().selected.clone();
                    w.reload(select);
                    let msg = if id.is_some() { tr("Default settings restored") } else { tr("Profiles restored") };
                    let warnings = warnings.unwrap_or_default();
                    w.toast(&if warnings.is_empty() { msg.to_string() } else { trf!("{msg}. Warnings: {warnings}", "{msg}. Avisos: {warnings}") });
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
                &trf!("Processor · {}/{} W", "Procesador · {}/{} W", int("pl1").unwrap_or(0), int("pl2").unwrap_or(0)),
                cpu_t,
            );
            if s["gpu_awake"].as_bool() == Some(true) {
                let gpu_t = int("gpu_temp");
                let pw = int("gpu_power").map(|p| format!(" · {p} W")).unwrap_or_default();
                w.st_gpu.set(&gpu_t.map(|t| format!("{t}°")).unwrap_or("—".into()), &trf!("Graphics{pw}", "Gráfica{pw}"), gpu_t);
                w.st_clock.set(
                    &int("gpu_clock").map(|c| c.to_string()).unwrap_or("—".into()),
                    &trf!("Core MHz · {}", "MHz núcleo · {}", s["gpu_pstate"].as_str().unwrap_or("")),
                    None,
                );
            } else {
                w.st_gpu.set(tr("Sleeping"), tr("Graphics idle"), None);
                w.st_clock.set("0", tr("Core MHz"), None);
            }
            let mode = match s["fan_mode"].as_str() {
                Some("auto") => "auto",
                Some("advanced") => tr("curve"),
                Some(other) => other,
                None => "",
            };
            let boost = match (s["cooler_boost"].as_bool(), s["auto_boost"].as_bool()) {
                (Some(true), Some(true)) => " · boost auto",
                (Some(true), _) => " · boost",
                _ => "",
            };
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

fn profile_summary(p: &Profile) -> String {
    let gpu = p.gpu.core_max.map(|c| format!("GPU ≤ {c} MHz"))
        .unwrap_or_else(|| tr("Factory GPU clocks").to_string());
    let fan = tr(if p.fans.mode == FanMode::Advanced { "custom curve" } else { "automatic fans" });
    let mut parts = vec![gpu, format!("CPU {}/{} W", p.cpu.pl1, p.cpu.pl2), fan.to_string()];
    if !p.cpu.turbo { parts.push(tr("turbo off").to_string()); }
    if p.fans.cooler_boost { parts.push("Cooler Boost".into()); }
    else if let Some(at) = p.fans.boost_at { parts.push(trf!("boost at {at}°", "boost a {at}°")); }
    parts.join(" · ")
}

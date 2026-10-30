//! Térmica: editor de perfiles de GPU, CPU y ventiladores (GTK4 + libadwaita).
//!
//! Opciones: `--barra` abre directamente el diálogo de la barra superior.
//! Desarrollo: `TERMICA_DEV=1` (habla con `termicad --dev`), `TERMICA_SCREENSHOT=archivo.png`.

mod client;
mod curve;
mod panel;
mod window;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "com.ikario.Termica";

const CSS: &str = "
.stat { padding: 12px 16px; }
.stat-value { font-size: 20pt; font-weight: 800; font-feature-settings: \"tnum\"; }
.stat-hot .stat-value { color: var(--warning-color); }
.stat-burn .stat-value { color: var(--error-color); }
.hero-title { font-size: 22pt; font-weight: 800; }
.curve-card { padding: 6px; }
";

fn main() -> glib::ExitCode {
    let open_bar = std::env::args().any(|a| a == "--barra");
    let app = adw::Application::builder().application_id(APP_ID).build();
    if std::env::var_os("TERMICA_SCREENSHOT").is_some() {
        // la captura de desarrollo no debe cederle el paso a una ventana ya abierta
        app.set_flags(gio::ApplicationFlags::NON_UNIQUE);
    }

    app.connect_startup(|app| {
        let display = gdk::Display::default().expect("sin pantalla");
        gtk::IconTheme::for_display(&display).add_search_path(panel::ext_dir().join("icons"));
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let quit = gio::ActionEntry::builder("quit").activate(|app: &adw::Application, _, _| app.quit()).build();
        app.add_action_entries([quit]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
    });

    let windows: Rc<RefCell<Option<Rc<window::Window>>>> = Rc::new(RefCell::new(None));
    let open_bar = std::cell::Cell::new(open_bar);
    app.connect_activate(move |app| {
        if let Some(w) = windows.borrow().as_ref() {
            w.win.present();
            return;
        }
        let client = match client::Client::new() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("No se pudo conectar con D-Bus: {e}");
                app.quit();
                return;
            }
        };
        let w = window::Window::new(app, client);
        if let Ok(path) = std::env::var("TERMICA_SCREENSHOT") {
            let h = std::env::var("TERMICA_SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(760);
            w.win.set_default_size(1040, h);
            let win = w.win.clone();
            glib::timeout_add_local_once(Duration::from_millis(2500), move || screenshot(&win, &path));
        }
        w.win.present();
        if open_bar.replace(false) {
            let w2 = w.clone();
            glib::idle_add_local_once(move || w2.open_panel_dialog());
        }
        *windows.borrow_mut() = Some(w);
    });

    // los argumentos propios no se pasan a GTK
    let args: Vec<String> = std::env::args().filter(|a| a != "--barra").collect();
    app.run_with_args(&args)
}

/// Uso de desarrollo: guarda la ventana como PNG y sale.
fn screenshot(win: &adw::ApplicationWindow, path: &str) {
    let paintable = gtk::WidgetPaintable::new(Some(win));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, win.width() as f64, win.height() as f64);
    if let (Some(node), Some(renderer)) = (snapshot.to_node(), win.native().and_then(|n| n.renderer())) {
        let _ = renderer.render_texture(node, None).save_to_png(path);
    }
    if let Some(app) = win.application() {
        app.quit();
    }
}

//! Diálogo «Barra superior»: qué datos muestra la extensión y si su icono está visible.

use crate::i18n::tr;

use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gio, glib};

pub const EXT_UUID: &str = "termica@ikario";
const SCHEMA_ID: &str = "org.gnome.shell.extensions.termica";

/// Igual que `extension/termica@ikario/metrics.js`: (id, título, icono, usa la NVIDIA).
const METRICS: &[(&str, &str, &str, bool)] = &[
    ("profile", "Active profile", "profile", false),
    ("cpu-temp", "CPU temperature", "cpu", false),
    ("gpu-temp", "GPU temperature", "gpu", false),
    ("cpu-power", "CPU power", "bolt", false),
    ("gpu-power", "GPU power", "bolt", true),
    ("cpu-fan-rpm", "Fan 1 (CPU), RPM", "fan", false),
    ("gpu-fan-rpm", "Fan 2 (GPU), RPM", "fan", false),
    ("cpu-fan-pct", "Fan 1 (CPU), %", "fan", false),
    ("gpu-fan-pct", "Fan 2 (GPU), %", "fan", false),
    ("gpu-clock", "GPU core clock", "speed", true),
    ("gpu-mem-clock", "GPU memory clock", "wave", true),
    ("gpu-util", "GPU utilization", "activity", true),
    ("vram", "GPU memory used (VRAM)", "vram", true),
    ("ram", "RAM used", "ram", false),
];

pub fn ext_dir() -> PathBuf {
    glib::user_data_dir().join("gnome-shell/extensions").join(EXT_UUID)
}

/// Configuración de la extensión, o `None` si no está instalada.
fn settings() -> Option<gio::Settings> {
    let dir = ext_dir().join("schemas");
    if !dir.join("gschemas.compiled").exists() {
        return None;
    }
    let src = gio::SettingsSchemaSource::from_directory(&dir, gio::SettingsSchemaSource::default().as_ref(), false)
        .ok()?;
    let schema = src.lookup(SCHEMA_ID, false)?;
    Some(gio::Settings::new_full(&schema, None::<&gio::SettingsBackend>, None))
}

fn extension_enabled() -> bool {
    gio::Settings::new("org.gnome.shell").strv("enabled-extensions").iter().any(|s| s == EXT_UUID)
}

/// Activa o desactiva la extensión pidiéndoselo a GNOME Shell.
async fn set_extension_enabled(enabled: bool) -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    bus.call_future(
        Some("org.gnome.Shell.Extensions"),
        "/org/gnome/Shell/Extensions",
        "org.gnome.Shell.Extensions",
        if enabled { "EnableExtension" } else { "DisableExtension" },
        Some(&(EXT_UUID,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        -1,
    )
    .await
    .map(|_| ())
}

/// Abre el diálogo. Devuelve `false` si la extensión no está instalada.
pub fn present(parent: &impl IsA<gtk::Widget>) -> bool {
    let Some(settings) = settings() else { return false };
    let dialog = adw::PreferencesDialog::builder().title(tr("Top bar")).search_enabled(true).build();
    let page = adw::PreferencesPage::builder().title(tr("Top bar")).icon_name("termica-symbolic").build();
    dialog.add(&page);

    // icono visible
    let top = adw::PreferencesGroup::new();
    let visible = adw::SwitchRow::builder()
        .title(tr("Show icon in top bar"))
        .subtitle(tr("Turning this off hides the icon; your profile stays active"))
        .active(extension_enabled())
        .build();
    visible.add_prefix(&gtk::Image::from_icon_name("termica-symbolic"));
    visible.connect_active_notify(|row| {
        let row = row.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = set_extension_enabled(row.is_active()).await {
                eprintln!("Could not change extension state: {}", e.message());
                row.set_active(!row.is_active());
            }
        });
    });
    top.add(&visible);
    page.add(&top);

    // qué mostrar
    let group = adw::PreferencesGroup::builder()
        .title(tr("What to show in the top bar"))
        .description(tr("Changes appear immediately. NVIDIA metrics show “—” while the GPU is asleep."))
        .build();
    let mut rows = vec![];
    for &(id, title, icon, nvidia) in METRICS {
        let row = adw::SwitchRow::builder()
            .title(tr(title))
            .subtitle(if nvidia { tr("Uses the NVIDIA GPU") } else { "" })
            .build();
        row.add_prefix(&gtk::Image::from_icon_name(&format!("termica-{icon}-symbolic")));
        let s = settings.clone();
        row.connect_active_notify(move |row| {
            let mut items: Vec<String> = s.strv("panel-items").iter().map(|x| x.to_string()).collect();
            items.retain(|x| x != id);
            if row.is_active() {
                items.push(id.to_string());
            }
            let ordered: Vec<&str> =
                METRICS.iter().map(|m| m.0).filter(|m| items.iter().any(|x| x == m)).collect();
            let current: Vec<String> = s.strv("panel-items").iter().map(|x| x.to_string()).collect();
            if ordered != current {
                let _ = s.set_strv("panel-items", ordered);
            }
        });
        group.add(&row);
        rows.push((id, row));
    }
    page.add(&group);

    let sync = {
        let rows = rows.clone();
        move |s: &gio::Settings| {
            let items = s.strv("panel-items");
            for (id, row) in &rows {
                let on = items.iter().any(|x| x == *id);
                if row.is_active() != on {
                    row.set_active(on);
                }
            }
        }
    };
    sync(&settings);
    let handler = settings.connect_changed(Some("panel-items"), move |s, _| sync(s));

    // formato
    let fmt = adw::PreferencesGroup::builder().title(tr("Format")).build();
    let labels = adw::SwitchRow::builder()
        .title(tr("Show short labels"))
        .subtitle(tr("“CPU”, “GPU”, “RAM”… next to each value"))
        .build();
    settings.bind("show-labels", &labels, "active").build();
    let color = adw::SwitchRow::builder()
        .title(tr("Color high temperatures"))
        .subtitle(tr("Yellow from 80 °C, red from 90 °C"))
        .build();
    settings.bind("color-temps", &color, "active").build();
    let interval = adw::SpinRow::with_range(1.0, 10.0, 1.0);
    interval.set_title(tr("Update every"));
    interval.set_subtitle(tr("Seconds"));
    settings.bind("interval", &interval, "value").build();
    fmt.add(&labels);
    fmt.add(&color);
    fmt.add(&interval);
    page.add(&fmt);

    let s = settings.clone();
    let handler = std::cell::Cell::new(Some(handler));
    dialog.connect_closed(move |_| {
        if let Some(h) = handler.take() {
            s.disconnect(h);
        }
    });
    dialog.present(Some(parent));
    true
}

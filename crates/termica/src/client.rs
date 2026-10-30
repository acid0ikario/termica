//! Conexión con el servicio `termicad` por D-Bus (con el bucle de GLib, sin runtime async extra).

use gtk::prelude::*;
use gtk::{gio, glib};
use termica_core::{BUS_NAME, INTERFACE, OBJECT_PATH};

#[derive(Clone)]
pub struct Client {
    proxy: gio::DBusProxy,
}

impl Client {
    pub fn new() -> Result<Self, glib::Error> {
        // TERMICA_DEV: hablar con un `termicad --dev` en el bus de sesión
        let bus = if std::env::var_os("TERMICA_DEV").is_some() { gio::BusType::Session } else { gio::BusType::System };
        let proxy = gio::DBusProxy::for_bus_sync(
            bus,
            gio::DBusProxyFlags::DO_NOT_LOAD_PROPERTIES,
            None,
            BUS_NAME,
            OBJECT_PATH,
            INTERFACE,
            gio::Cancellable::NONE,
        )?;
        Ok(Self { proxy })
    }

    pub fn available(&self) -> bool {
        self.proxy.name_owner().is_some()
    }

    /// Llama a un método con un argumento de texto opcional y devuelve su resultado de texto.
    pub async fn call(&self, method: &str, arg: Option<&str>) -> Result<Option<String>, String> {
        let params = arg.map(|a| (a,).to_variant());
        self.call_variant(method, params).await
    }

    pub async fn call_variant(&self, method: &str, params: Option<glib::Variant>) -> Result<Option<String>, String> {
        match self
            .proxy
            .call_future(method, params.as_ref(), gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION, 30_000)
            .await
        {
            Ok(v) => Ok(v.try_child_value(0).and_then(|c| c.get::<String>())),
            Err(e) => Err(clean_error(&e)),
        }
    }

    pub fn connect_signal(&self, f: impl Fn(&str) + 'static) {
        // g-signal(sender_name, signal_name, parameters); `connect_local` no exige Send
        self.proxy.connect_local("g-signal", false, move |values| {
            if let Some(Ok(name)) = values.get(2).map(|v| v.get::<String>()) {
                f(&name);
            }
            None
        });
    }

    pub fn connect_available(&self, f: impl Fn(bool) + 'static) {
        self.proxy.connect_notify_local(Some("g-name-owner"), move |p, _| f(p.name_owner().is_some()));
    }
}

/// «GDBus.Error:com.ikario.Termica1.Error.Invalid: mensaje» → «mensaje»
fn clean_error(e: &glib::Error) -> String {
    let msg = e.message();
    match msg.strip_prefix("GDBus.Error:") {
        Some(rest) => rest.split_once(": ").map(|(_, m)| m.to_string()).unwrap_or_else(|| rest.to_string()),
        None => msg.to_string(),
    }
}

//! API de D-Bus (`com.ikario.Termica1`), estado compartido y autorización con polkit.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Map, Value, json};
use termica_core::{POLKIT_ACTION, Profile, Store};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{Connection, Variant};

use crate::hw::{self, Part};

pub struct Config {
    pub dev: bool,
    pub conf_dir: PathBuf,
}

impl Config {
    pub fn profiles_file(&self) -> PathBuf {
        self.conf_dir.join("profiles.json")
    }

    pub fn allow_unknown_ec(&self) -> PathBuf {
        self.conf_dir.join("allow-unknown-ec")
    }
}

pub struct State {
    pub store: Store,
    /// Cambios rápidos desde la barra (cooler_boost, turbo), hasta el próximo cambio de perfil.
    pub overrides: BTreeMap<String, bool>,
    energy: Option<(Instant, i64)>,
    cpu_watts: Option<f64>,
    nv_cache: Option<(Instant, Map<String, Value>)>,
}

impl State {
    pub fn new(store: Store) -> Self {
        Self { store, overrides: BTreeMap::new(), energy: None, cpu_watts: None, nv_cache: None }
    }
}

pub type Shared = Arc<Mutex<State>>;

#[derive(zbus::DBusError, Debug)]
#[zbus(prefix = "com.ikario.Termica1.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    Invalid(String),
    NotAuthorized(String),
    Failed(String),
}

// ------------------------------------------------------------------ persistencia

pub fn load_store(cfg: &Config) -> Store {
    let path = cfg.profiles_file();
    if let Ok(text) = std::fs::read_to_string(&path) {
        match serde_json::from_str::<Store>(&text).map_err(|e| e.to_string()).and_then(Store::validated) {
            Ok(store) => return store,
            Err(e) => log::error!("No se pudo leer {}: {e}. Uso los perfiles iniciales.", path.display()),
        }
    }
    let store = serde_json::from_str::<Store>(termica_core::DEFAULT_PROFILES_JSON)
        .expect("perfiles iniciales")
        .validated()
        .expect("perfiles iniciales válidos");
    if let Err(e) = save_store(cfg, &store) {
        log::error!("No se pudo guardar {}: {e}", path.display());
    }
    store
}

pub fn save_store(cfg: &Config, store: &Store) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(&cfg.conf_dir).map_err(|e| e.to_string())?;
    let path = cfg.profiles_file();
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(store).map_err(|e| e.to_string())? + "\n";
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ aplicar

/// Aplica el perfil activo y luego los cambios rápidos (en un hilo aparte: toca hardware).
pub async fn apply_active(state: &Shared, cfg: &Arc<Config>, parts: &'static [Part]) -> Vec<String> {
    let (profile, overrides) = {
        let s = state.lock().unwrap();
        (s.store.active_profile().clone(), s.overrides.clone())
    };
    let cfg = cfg.clone();
    tokio::task::spawn_blocking(move || {
        let warnings = hw::apply_profile(&profile, parts, &cfg.allow_unknown_ec(), cfg.dev);
        if !cfg.dev {
            for (key, value) in overrides {
                let _ = apply_quick(&key, value);
            }
        }
        warnings
    })
    .await
    .unwrap_or_default()
}

fn apply_quick(key: &str, value: bool) -> Result<(), String> {
    match key {
        "cooler_boost" => hw::set_cooler_boost(value),
        "turbo" => hw::set_turbo(value),
        _ => Err("Ajuste desconocido".into()),
    }
}

// ------------------------------------------------------------------ polkit

async fn authorize(conn: &Connection, hdr: &Header<'_>, cfg: &Config) -> Result<(), Error> {
    if cfg.dev {
        return Ok(());
    }
    let sender = hdr.sender().ok_or_else(|| Error::NotAuthorized("Remitente desconocido".into()))?.to_string();
    let authority = zbus::Proxy::new(
        conn,
        "org.freedesktop.PolicyKit1",
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
    )
    .await?;
    let mut subject_details: HashMap<&str, Variant<'_>> = HashMap::new();
    subject_details.insert("name", Variant::from(sender.as_str()));
    let details: HashMap<&str, &str> = HashMap::new();
    let flags: u32 = 1; // permitir que polkit pregunte si hace falta
    let ((authorized, _challenge, _info),): ((bool, bool, HashMap<String, String>),) = authority
        .call("CheckAuthorization", &(("system-bus-name", subject_details), POLKIT_ACTION, details, flags, ""))
        .await?;
    if authorized { Ok(()) } else { Err(Error::NotAuthorized("No tienes permiso para cambiar perfiles".into())) }
}

// ------------------------------------------------------------------ interfaz

pub struct Termica {
    pub state: Shared,
    pub cfg: Arc<Config>,
}

impl Termica {
    fn cpu_power(s: &mut State) -> Option<f64> {
        let now = Instant::now();
        let e = hw::read_int(format!("{}/energy_uj", hw::RAPL));
        if let (Some((then, last)), Some(e)) = (s.energy, e) {
            let dt = now.duration_since(then).as_secs_f64();
            if e >= last && dt > 0.3 {
                s.cpu_watts = Some(((e - last) as f64 / 1e6 / dt * 10.0).round() / 10.0);
            }
        }
        if let Some(e) = e {
            s.energy = Some((now, e));
        }
        s.cpu_watts
    }

    /// Datos de sysfs: baratos y sin despertar la GPU.
    fn light(&self) -> Map<String, Value> {
        let msi = hw::MSI;
        let fan = hw::find_hwmon(None, Some("fan1_input"));
        let mut m = Map::new();
        m.insert("cpu_temp".into(), json!(hw::cpu_temp()));
        m.insert("gpu_temp".into(), json!(hw::read_int(format!("{msi}/gpu/realtime_temperature"))));
        m.insert("cpu_fan".into(), json!(hw::read_int(format!("{msi}/cpu/realtime_fan_speed"))));
        m.insert("gpu_fan".into(), json!(hw::read_int(format!("{msi}/gpu/realtime_fan_speed"))));
        m.insert("cpu_rpm".into(), json!(fan.as_ref().and_then(|f| hw::read_int(f.join("fan1_input")))));
        m.insert("gpu_rpm".into(), json!(fan.as_ref().and_then(|f| hw::read_int(f.join("fan2_input")))));
        m.insert("turbo".into(), json!(hw::read(hw::NO_TURBO).as_deref() == Some("0")));
        let meminfo: HashMap<String, f64> = hw::read("/proc/meminfo")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                Some((k.to_string(), v.split_whitespace().next()?.parse().ok()?))
            })
            .collect();
        if let Some(total) = meminfo.get("MemTotal") {
            let avail = meminfo.get("MemAvailable").copied().unwrap_or(0.0);
            m.insert("ram_used".into(), json!(((total - avail) / 1_048_576.0 * 10.0).round() / 10.0));
            m.insert("ram_total".into(), json!((total / 1_048_576.0 * 10.0).round() / 10.0));
        }
        m.insert("gpu_awake".into(), json!(hw::gpu_awake()));
        let mut s = self.state.lock().unwrap();
        m.insert("cpu_power".into(), json!(Self::cpu_power(&mut s)));
        m.insert("active".into(), json!(s.store.active));
        m.insert("profile_name".into(), json!(s.store.active_profile().name));
        m
    }

    /// Datos de la NVIDIA con una pequeña caché (varios clientes pueden preguntar a la vez).
    async fn nvidia(&self) -> Option<Map<String, Value>> {
        {
            let s = self.state.lock().unwrap();
            if let Some((t, data)) = &s.nv_cache
                && t.elapsed().as_secs_f64() < 1.5
            {
                return Some(data.clone());
            }
        }
        let data = tokio::task::spawn_blocking(hw::nvidia_status).await.ok().flatten()?;
        self.state.lock().unwrap().nv_cache = Some((Instant::now(), data.clone()));
        Some(data)
    }
}

#[zbus::interface(name = "com.ikario.Termica1")]
impl Termica {
    async fn get_profiles(&self) -> String {
        serde_json::to_string(&self.state.lock().unwrap().store).unwrap()
    }

    async fn get_temps(&self) -> String {
        Value::Object(self.light()).to_string()
    }

    async fn get_status(&self) -> String {
        let mut m = self.light();
        let rapl = hw::RAPL;
        let msi = hw::MSI;
        m.insert("fan_mode".into(), json!(hw::read(format!("{msi}/fan_mode"))));
        m.insert("cooler_boost".into(), json!(hw::read(format!("{msi}/cooler_boost")).as_deref() == Some("on")));
        m.insert("shift_mode".into(), json!(hw::read(format!("{msi}/shift_mode"))));
        for (key, n) in [("pl1", 0), ("pl2", 1)] {
            let uw = hw::read_int(format!("{rapl}/constraint_{n}_power_limit_uw")).unwrap_or(0);
            m.insert(key.into(), json!(uw / 1_000_000));
        }
        if m.get("gpu_awake") == Some(&Value::Bool(true))
            && let Some(nv) = self.nvidia().await
        {
            m.extend(nv);
        }
        m.insert("overrides".into(), json!(self.state.lock().unwrap().overrides));
        Value::Object(m).to_string()
    }

    async fn activate(
        &self,
        id: String,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<String, Error> {
        authorize(conn, &hdr, &self.cfg).await?;
        {
            let mut s = self.state.lock().unwrap();
            if s.store.get(&id).is_none() {
                return Err(Error::Invalid("Ese perfil no existe".into()));
            }
            s.store.active = id.clone();
            s.overrides.clear();
            save_store(&self.cfg, &s.store).map_err(Error::Failed)?;
        }
        let warnings = apply_active(&self.state, &self.cfg, &hw::ALL).await;
        emitter.active_changed(&id).await?;
        Ok(warnings.join("\n"))
    }

    async fn save_profile(
        &self,
        json: String,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<String, Error> {
        authorize(conn, &hdr, &self.cfg).await?;
        let incoming: Profile =
            serde_json::from_str(&json).map_err(|e| Error::Invalid(format!("Perfil inválido: {e}")))?;
        let mut p = incoming.validated().map_err(Error::Invalid)?;
        let is_active = {
            let mut s = self.state.lock().unwrap();
            match s.store.profiles.iter().position(|x| !p.id.is_empty() && x.id == p.id) {
                Some(i) => s.store.profiles[i] = p.clone(),
                None => {
                    p.id = s.store.new_id(&p.name);
                    s.store.profiles.push(p.clone());
                }
            }
            save_store(&self.cfg, &s.store).map_err(Error::Failed)?;
            s.store.active == p.id
        };
        let warnings = if is_active { apply_active(&self.state, &self.cfg, &hw::ALL).await } else { vec![] };
        emitter.profiles_changed().await?;
        Ok(json!({"id": p.id, "warnings": warnings}).to_string())
    }

    async fn delete_profile(
        &self,
        id: String,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        authorize(conn, &hdr, &self.cfg).await?;
        {
            let mut s = self.state.lock().unwrap();
            if s.store.get(&id).is_none() {
                return Err(Error::Invalid("Ese perfil no existe".into()));
            }
            if s.store.active == id {
                return Err(Error::Invalid("No se puede borrar el perfil activo".into()));
            }
            if s.store.profiles.len() <= 1 {
                return Err(Error::Invalid("Debe quedar al menos un perfil".into()));
            }
            s.store.profiles.retain(|p| p.id != id);
            save_store(&self.cfg, &s.store).map_err(Error::Failed)?;
        }
        emitter.profiles_changed().await?;
        Ok(())
    }

    /// Cambio rápido (`cooler_boost` o `turbo`) que dura hasta el próximo cambio de perfil.
    async fn set_quick(
        &self,
        key: String,
        value: bool,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Error> {
        authorize(conn, &hdr, &self.cfg).await?;
        if key != "cooler_boost" && key != "turbo" {
            return Err(Error::Invalid("Ajuste desconocido".into()));
        }
        if self.cfg.dev {
            log::info!("[dev] {key} = {value}");
        } else {
            apply_quick(&key, value).map_err(Error::Failed)?;
        }
        self.state.lock().unwrap().overrides.insert(key.clone(), value);
        emitter.quick_changed(&key, value).await?;
        Ok(())
    }

    #[zbus(signal)]
    async fn active_changed(emitter: &SignalEmitter<'_>, id: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn profiles_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn quick_changed(emitter: &SignalEmitter<'_>, key: &str, value: bool) -> zbus::Result<()>;
}

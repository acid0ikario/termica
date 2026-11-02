//! Tipos y reglas compartidas entre el servicio `termicad` y la app `termica`.
//!
//! El formato JSON es el mismo de la versión en Python, así que
//! `/etc/termica/profiles.json` sigue siendo compatible.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

pub const BUS_NAME: &str = "com.ikario.Termica1";
pub const OBJECT_PATH: &str = "/com/ikario/Termica1";
pub const INTERFACE: &str = "com.ikario.Termica1";
pub const POLKIT_ACTION: &str = "com.ikario.termica.manage";

pub const GPU_CORE_MIN: u32 = 210;
pub const GPU_CORE_LIMIT_MIN: u32 = 300;
pub const GPU_CORE_MAX: u32 = 2100;
pub const GPU_MEM_MIN: u32 = 405;
pub const GPU_MEM_STEPS: [u32; 3] = [810, 5501, 6001];
pub const PL1_RANGE: (u32, u32) = (5, 115);
pub const PL2_RANGE: (u32, u32) = (5, 200);
pub const TEMP_RANGE: (u32, u32) = (30, 100);
/// El EC usa una escala de velocidad que llega a 150.
pub const SPEED_MAX: u32 = 150;
/// Seguridad: el último tramo de la curva nunca por debajo de esto.
pub const LAST_SPEED_MIN: u32 = 70;
pub const NAME_MAX: usize = 40;
/// Cooler Boost automático: temperatura a la que se enciende y segundos que se mantiene.
pub const BOOST_TEMP_RANGE: (u32, u32) = (60, 95);
pub const BOOST_SECS_RANGE: (u32, u32) = (5, 120);
pub const BOOST_SECS_DEFAULT: u32 = 30;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FanMode {
    Auto,
    Advanced,
}

impl FanMode {
    pub fn as_str(self) -> &'static str {
        match self {
            FanMode::Auto => "auto",
            FanMode::Advanced => "advanced",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShiftMode {
    Eco,
    Comfort,
    Turbo,
}

impl ShiftMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ShiftMode::Eco => "eco",
            ShiftMode::Comfort => "comfort",
            ShiftMode::Turbo => "turbo",
        }
    }
}

/// Curva escalonada del EC: 6 temperaturas de cambio y 7 velocidades.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Curve {
    pub temps: Vec<u32>,
    pub speeds: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Curves {
    pub cpu: Curve,
    pub gpu: Curve,
}

impl Default for Curves {
    fn default() -> Self {
        factory_curves()
    }
}

/// Curva que trae el equipo al arrancar (leída antes de cualquier cambio).
pub fn factory_curves() -> Curves {
    Curves {
        cpu: Curve { temps: vec![68, 78, 85, 90, 94, 97], speeds: vec![0, 30, 38, 55, 75, 100, 120] },
        gpu: Curve { temps: vec![60, 70, 76, 82, 86, 90], speeds: vec![0, 30, 38, 50, 65, 85, 110] },
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Gpu {
    /// `None` = velocidad de fábrica.
    #[serde(default)]
    pub core_max: Option<u32>,
    #[serde(default)]
    pub mem_max: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Cpu {
    pub pl1: u32,
    pub pl2: u32,
    #[serde(default = "yes")]
    pub turbo: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Fans {
    pub mode: FanMode,
    #[serde(default)]
    pub cooler_boost: bool,
    /// Cooler Boost automático: se enciende cuando la CPU o la GPU llegan a esta temperatura
    /// y se apaga `boost_secs` segundos después de que baje de ella. `None` = desactivado.
    #[serde(default)]
    pub boost_at: Option<u32>,
    #[serde(default = "boost_secs_default")]
    pub boost_secs: u32,
    #[serde(default)]
    pub curves: Curves,
}

fn boost_secs_default() -> u32 {
    BOOST_SECS_DEFAULT
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub gpu: Gpu,
    pub cpu: Cpu,
    pub fans: Fans,
    #[serde(default)]
    pub shift_mode: Option<ShiftMode>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Store {
    pub active: String,
    pub profiles: Vec<Profile>,
}

impl Store {
    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn active_profile(&self) -> &Profile {
        self.get(&self.active).unwrap_or(&self.profiles[0])
    }

    /// Valida todos los perfiles y corrige un `active` que no exista.
    pub fn validated(mut self) -> Result<Self, String> {
        if self.profiles.is_empty() {
            return Err("No profiles available".into());
        }
        for p in &mut self.profiles {
            let id = p.id.clone();
            *p = p.validated()?;
            p.id = id;
        }
        if self.get(&self.active).is_none() {
            self.active = self.profiles[0].id.clone();
        }
        Ok(self)
    }

    /// Un id libre a partir del nombre.
    pub fn new_id(&self, name: &str) -> String {
        let base = slugify(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.get(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        id
    }
}

fn check_range(v: u32, (lo, hi): (u32, u32), what: &str) -> Result<u32, String> {
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{what}: must be between {lo} and {hi}"))
    }
}

impl Curve {
    pub fn validate(&self, what: &str) -> Result<(), String> {
        if self.temps.len() != 6 || self.speeds.len() != 7 {
            return Err(format!("{what}: expected 6 temperatures and 7 speeds"));
        }
        for &t in &self.temps {
            check_range(t, TEMP_RANGE, &format!("{what}, temperature"))?;
        }
        for &s in &self.speeds {
            check_range(s, (0, SPEED_MAX), &format!("{what}, speed"))?;
        }
        if self.temps.windows(2).any(|w| w[1] <= w[0]) {
            return Err(format!("{what}: temperatures must increase"));
        }
        if self.speeds.windows(2).any(|w| w[1] < w[0]) {
            return Err(format!("{what}: speed cannot decrease as temperature increases"));
        }
        if self.speeds[6] < LAST_SPEED_MIN {
            return Err(format!("{what}: for safety the last point must be at least {LAST_SPEED_MIN}"));
        }
        Ok(())
    }
}

impl Profile {
    /// Devuelve una copia normalizada, o un mensaje que explica qué está mal.
    pub fn validated(&self) -> Result<Profile, String> {
        let mut p = self.clone();
        p.name = p.name.trim().chars().take(NAME_MAX).collect();
        if p.name.is_empty() {
            return Err("The profile needs a name".into());
        }
        if let Some(core) = p.gpu.core_max {
            check_range(core, (GPU_CORE_LIMIT_MIN, GPU_CORE_MAX), "GPU, maximum core clock")?;
        }
        if let Some(mem) = p.gpu.mem_max
            && !GPU_MEM_STEPS.contains(&mem)
        {
            return Err(format!("GPU, maximum memory clock: allowed values {GPU_MEM_STEPS:?}"));
        }
        check_range(p.cpu.pl1, PL1_RANGE, "CPU, sustained power (PL1)")?;
        check_range(p.cpu.pl2, PL2_RANGE, "CPU, burst power (PL2)")?;
        if p.cpu.pl2 < p.cpu.pl1 {
            return Err("CPU: burst power (PL2) cannot be lower than sustained power (PL1)".into());
        }
        if let Some(at) = p.fans.boost_at {
            check_range(at, BOOST_TEMP_RANGE, "Automatic Cooler Boost, temperature")?;
        }
        check_range(p.fans.boost_secs, BOOST_SECS_RANGE, "Automatic Cooler Boost, seconds")?;
        p.fans.curves.cpu.validate("CPU curve")?;
        p.fans.curves.gpu.validate("GPU curve")?;
        Ok(p)
    }

    /// Resumen corto para listas: «GPU ≤ 800 MHz · CPU 80/200 W · curva propia».
    pub fn summary(&self) -> String {
        let mut parts = vec![
            match self.gpu.core_max {
                Some(c) => format!("GPU ≤ {c} MHz"),
                None => "Factory GPU clocks".into(),
            },
            format!("CPU {}/{} W", self.cpu.pl1, self.cpu.pl2),
            match self.fans.mode {
                FanMode::Advanced => "custom curve".into(),
                FanMode::Auto => "automatic fans".into(),
            },
        ];
        if !self.cpu.turbo {
            parts.push("turbo off".into());
        }
        if self.fans.cooler_boost {
            parts.push("cooler boost".into());
        } else if let Some(at) = self.fans.boost_at {
            parts.push(format!("boost at {at}°"));
        }
        parts.join(" · ")
    }
}

pub fn slugify(text: &str) -> String {
    let ascii: String = text.nfkd().filter(|c| c.is_ascii()).collect::<String>().to_lowercase();
    let mut out = String::new();
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "perfil".into() } else { out }
}

/// Perfiles iniciales (los mismos de `data/default-profiles.json`).
pub const DEFAULT_PROFILES_JSON: &str = include_str!("../../../data/default-profiles.json");

/// Los perfiles que trae Térmica (Silencioso, Balanceado, Rendimiento y Fábrica) con sus valores por defecto.
pub fn default_store() -> Store {
    serde_json::from_str::<Store>(DEFAULT_PROFILES_JSON)
        .expect("default profiles")
        .validated()
        .expect("valid default profiles")
}

/// Valores por defecto de un perfil incluido, o `None` si lo creó el usuario.
pub fn default_profile(id: &str) -> Option<Profile> {
    default_store().get(id).cloned()
}

impl Store {
    /// Devuelve los perfiles incluidos a sus valores por defecto (`None` = todos).
    /// Los perfiles creados por el usuario no se tocan; un perfil incluido que se borró vuelve a aparecer.
    pub fn restore_defaults(&mut self, id: Option<&str>) -> Result<Vec<String>, String> {
        let defaults = default_store();
        let targets: Vec<&Profile> = match id {
            Some(id) => vec![defaults.get(id).ok_or("This profile has no default settings")?],
            None => defaults.profiles.iter().collect(),
        };
        let mut restored = vec![];
        for d in targets {
            match self.profiles.iter_mut().find(|p| p.id == d.id) {
                Some(p) => *p = d.clone(),
                None => self.profiles.push(d.clone()),
            }
            restored.push(d.id.clone());
        }
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        serde_json::from_str::<Store>(DEFAULT_PROFILES_JSON).unwrap().validated().unwrap()
    }

    #[test]
    fn defaults_are_valid() {
        let s = store();
        assert_eq!(s.active, "silencioso");
        assert_eq!(s.profiles.len(), 4);
        assert_eq!(s.get("fabrica").unwrap().gpu.core_max, None);
    }

    #[test]
    fn rejects_pl2_below_pl1() {
        let mut p = store().profiles[0].clone();
        p.cpu.pl2 = p.cpu.pl1 - 1;
        assert!(p.validated().unwrap_err().contains("PL2"));
    }

    #[test]
    fn rejects_unsafe_curve() {
        let mut p = store().profiles[0].clone();
        p.fans.curves.cpu.speeds = vec![0, 10, 20, 30, 40, 50, 60];
        assert!(p.validated().unwrap_err().contains("safety"));
        p.fans.curves.cpu.speeds = vec![0, 50, 40, 60, 80, 100, 100];
        assert!(p.validated().unwrap_err().contains("cannot decrease"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Fábrica"), "fabrica");
        assert_eq!(slugify("Render Blender!"), "render-blender");
        assert_eq!(store().new_id("Silencioso"), "silencioso-2");
    }

    #[test]
    fn restore_defaults() {
        let mut s = store();
        s.profiles[0].gpu.core_max = Some(1900);
        s.profiles[0].name = "Otro nombre".into();
        s.profiles.retain(|p| p.id != "fabrica");
        let mut mine = s.profiles[1].clone();
        mine.id = "mio".into();
        s.profiles.push(mine.clone());

        assert_eq!(s.restore_defaults(Some("silencioso")).unwrap(), ["silencioso"]);
        assert_eq!(s.get("silencioso"), default_profile("silencioso").as_ref());
        assert!(s.get("fabrica").is_none());

        s.restore_defaults(None).unwrap();
        assert!(s.get("fabrica").is_some(), "un perfil incluido borrado vuelve");
        assert_eq!(s.get("mio"), Some(&mine), "los perfiles propios no se tocan");
        assert!(s.restore_defaults(Some("mio")).is_err());
    }

    #[test]
    fn legacy_profile_without_turbo() {
        let json = r#"{"name":"x","cpu":{"pl1":10,"pl2":20},"fans":{"mode":"auto"}}"#;
        let p: Profile = serde_json::from_str(json).unwrap();
        assert!(p.cpu.turbo);
        assert_eq!(p.fans.curves, factory_curves());
    }

    #[test]
    fn auto_boost() {
        // los perfiles guardados antes de esta opción la traen apagada
        let json = r#"{"name":"x","cpu":{"pl1":10,"pl2":20},"fans":{"mode":"auto"}}"#;
        let p: Profile = serde_json::from_str(json).unwrap();
        assert_eq!((p.fans.boost_at, p.fans.boost_secs), (None, BOOST_SECS_DEFAULT));

        let mut p = store().profiles[0].clone();
        assert_eq!(p.fans.boost_at, Some(83));
        p.fans.boost_at = Some(99);
        assert!(p.validated().is_err());
        p.fans.boost_at = Some(80);
        p.fans.boost_secs = 0;
        assert!(p.validated().is_err());
    }
}

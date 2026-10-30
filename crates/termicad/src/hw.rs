//! Acceso al hardware: sysfs (msi-ec, RAPL, intel_pstate), el EC y la NVIDIA (NVML).

use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::enums::device::GpuLockedClocksSetting;
use serde_json::{Map, Value, json};
use termica_core::{FanMode, GPU_CORE_MIN, GPU_MEM_MIN, Profile, factory_curves};

pub const EC_IO: &str = "/sys/kernel/debug/ec/ec0/io";
pub const MSI: &str = "/sys/devices/platform/msi-ec";
pub const RAPL: &str = "/sys/class/powercap/intel-rapl:0";
pub const GPU_PCI: &str = "/sys/bus/pci/devices/0000:01:00.0";
pub const NO_TURBO: &str = "/sys/devices/system/cpu/intel_pstate/no_turbo";

/// Direcciones del EC del MSI Stealth 15M (firmware 15B1EMS1): (temperaturas x6, velocidades x7).
/// En otros modelos pueden ser distintas, así que solo se escriben si el firmware coincide.
const EC_CPU: (u64, u64) = (0x6A, 0x72);
const EC_GPU: (u64, u64) = (0x82, 0x8A);
const SUPPORTED_EC_FW: &[&str] = &["15B1EMS1"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Fans,
    Cpu,
    Gpu,
}

pub const ALL: [Part; 3] = [Part::Fans, Part::Cpu, Part::Gpu];

impl Part {
    fn label(self) -> &'static str {
        match self {
            Part::Fans => "ventiladores",
            Part::Cpu => "procesador",
            Part::Gpu => "gráfica",
        }
    }
}

// ------------------------------------------------------------------ sysfs

pub fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub fn read_int(path: impl AsRef<Path>) -> Option<i64> {
    read(path)?.parse().ok()
}

fn write(path: impl AsRef<Path>, value: impl ToString) -> Result<(), String> {
    let path = path.as_ref();
    fs::write(path, value.to_string()).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn find_hwmon(name: Option<&str>, file: Option<&str>) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir("/sys/class/hwmon").ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.into_iter().find(|d| {
        name.is_none_or(|n| read(d.join("name")).as_deref() == Some(n))
            && file.is_none_or(|f| d.join(f).exists())
    })
}

pub fn gpu_awake() -> bool {
    read(format!("{GPU_PCI}/power/runtime_status")).as_deref() != Some("suspended")
}

pub fn cpu_temp() -> Option<i64> {
    find_hwmon(Some("coretemp"), None)
        .and_then(|h| read_int(h.join("temp1_input")))
        .map(|t| (t as f64 / 1000.0).round() as i64)
        .or_else(|| read_int(format!("{MSI}/cpu/realtime_temperature")))
}

// ------------------------------------------------------------------ EC

fn ensure_ec() {
    if !Path::new(EC_IO).exists() || read("/sys/module/ec_sys/parameters/write_support").as_deref() != Some("Y") {
        let _ = Command::new("modprobe").args(["-r", "ec_sys"]).status();
        let _ = Command::new("modprobe").args(["ec_sys", "write_support=1"]).status();
    }
    if !Path::new(MSI).is_dir() {
        let _ = Command::new("modprobe").arg("msi_ec").status();
    }
}

pub fn ec_supported(allow_unknown: &Path) -> bool {
    let fw = read(format!("{MSI}/fw_version")).unwrap_or_default();
    SUPPORTED_EC_FW.iter().any(|p| fw.starts_with(p)) || allow_unknown.exists()
}

fn ec_write(addr: u64, values: &[u32]) -> Result<(), String> {
    let bytes: Vec<u8> = values.iter().map(|&v| v as u8).collect();
    let mut f = fs::OpenOptions::new().write(true).open(EC_IO).map_err(|e| format!("EC: {e}"))?;
    f.seek(SeekFrom::Start(addr)).and_then(|_| f.write_all(&bytes)).map_err(|e| format!("EC: {e}"))
}

// ------------------------------------------------------------------ aplicar

fn apply_fans(p: &Profile, allow_unknown: &Path) -> Result<(), String> {
    ensure_ec();
    if ec_supported(allow_unknown) {
        let curves = match p.fans.mode {
            FanMode::Advanced => p.fans.curves.clone(),
            FanMode::Auto => factory_curves(),
        };
        for (curve, (t_addr, s_addr)) in [(&curves.cpu, EC_CPU), (&curves.gpu, EC_GPU)] {
            ec_write(t_addr, &curve.temps)?;
            ec_write(s_addr, &curve.speeds)?;
        }
    } else if p.fans.mode == FanMode::Advanced {
        return Err(format!(
            "firmware {} no verificado: no se escribe la curva (ver README, {})",
            read(format!("{MSI}/fw_version")).unwrap_or_default(),
            allow_unknown.display()
        ));
    }
    write(format!("{MSI}/fan_mode"), p.fans.mode.as_str())?;
    set_cooler_boost(p.fans.cooler_boost)?;
    if let Some(shift) = p.shift_mode {
        write(format!("{MSI}/shift_mode"), shift.as_str())?;
    }
    Ok(())
}

fn apply_cpu(p: &Profile) -> Result<(), String> {
    let (pl1, pl2) = (p.cpu.pl1 as i64 * 1_000_000, p.cpu.pl2 as i64 * 1_000_000);
    let current_pl1 = read_int(format!("{RAPL}/constraint_0_power_limit_uw")).unwrap_or(0);
    // nunca dejar PL2 < PL1 entre escrituras
    write(format!("{RAPL}/constraint_1_power_limit_uw"), pl2.max(current_pl1))?;
    write(format!("{RAPL}/constraint_0_power_limit_uw"), pl1)?;
    write(format!("{RAPL}/constraint_1_power_limit_uw"), pl2)?;
    set_turbo(p.cpu.turbo)
}

fn apply_gpu(p: &Profile) -> Result<(), String> {
    let nvml = Nvml::init().map_err(|e| format!("NVML: {e}"))?;
    let mut dev = nvml.device_by_index(0).map_err(|e| format!("NVML: {e}"))?;
    match p.gpu.core_max {
        Some(max) => dev.set_gpu_locked_clocks(GpuLockedClocksSetting::Numeric {
            min_clock_mhz: GPU_CORE_MIN,
            max_clock_mhz: max,
        }),
        None => dev.reset_gpu_locked_clocks(),
    }
    .map_err(|e| format!("núcleo: {e}"))?;
    match p.gpu.mem_max {
        Some(max) => dev.set_mem_locked_clocks(GPU_MEM_MIN, max),
        None => dev.reset_mem_locked_clocks(),
    }
    .map_err(|e| format!("memoria: {e}"))
}

pub fn set_turbo(on: bool) -> Result<(), String> {
    write(NO_TURBO, if on { 0 } else { 1 })
}

pub fn set_cooler_boost(on: bool) -> Result<(), String> {
    write(format!("{MSI}/cooler_boost"), if on { "on" } else { "off" })
}

/// Aplica el perfil parte por parte. Devuelve avisos (vacío si todo salió bien).
pub fn apply_profile(p: &Profile, parts: &[Part], allow_unknown: &Path, dev: bool) -> Vec<String> {
    let names: Vec<_> = parts.iter().map(|p| p.label()).collect();
    if dev {
        log::info!("[dev] aplicaría «{}» ({}): {}", p.name, names.join(", "), serde_json::to_string(p).unwrap());
        return vec![];
    }
    let mut warnings = vec![];
    for &part in parts {
        let result = match part {
            Part::Fans => apply_fans(p, allow_unknown),
            Part::Cpu => apply_cpu(p),
            Part::Gpu => apply_gpu(p),
        };
        if let Err(e) = result {
            log::warn!("Error aplicando {}: {e}", part.label());
            warnings.push(format!("{}: {e}", part.label()));
        }
    }
    log::info!("Perfil «{}» aplicado ({})", p.name, names.join(", "));
    warnings
}

// ------------------------------------------------------------------ lectura NVIDIA

/// ¿La GPU va más rápido que el límite del perfil? (señal de que el driver lo olvidó)
pub fn gpu_over_limit(gpu: &termica_core::Gpu) -> bool {
    const MARGIN: u32 = 30;
    if gpu.core_max.is_none() && gpu.mem_max.is_none() {
        return false;
    }
    let Ok(nvml) = Nvml::init() else { return false };
    let Ok(dev) = nvml.device_by_index(0) else { return false };
    let over = |clock, limit: Option<u32>| match (limit, dev.clock_info(clock)) {
        (Some(max), Ok(now)) => now > max + MARGIN,
        _ => false,
    };
    over(Clock::Graphics, gpu.core_max) || over(Clock::Memory, gpu.mem_max)
}

fn pstate_name(state: nvml_wrapper::enum_wrappers::device::PerformanceState) -> String {
    let debug = format!("{state:?}");
    const NAMES: [&str; 16] = [
        "Zero", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine", "Ten", "Eleven", "Twelve",
        "Thirteen", "Fourteen", "Fifteen",
    ];
    NAMES.iter().position(|n| *n == debug).map(|i| format!("P{i}")).unwrap_or_else(|| "P?".into())
}

/// Datos de la NVIDIA vía NVML. Se abre y se cierra en cada lectura para no impedir que la GPU duerma.
pub fn nvidia_status() -> Option<Map<String, Value>> {
    let nvml = Nvml::init().ok()?;
    let dev = nvml.device_by_index(0).ok()?;
    let mut m = Map::new();
    let mut put = |k: &str, v: Value| {
        m.insert(k.into(), v);
    };
    put("gpu_temp", json!(dev.temperature(TemperatureSensor::Gpu).ok()));
    put("gpu_clock", json!(dev.clock_info(Clock::Graphics).ok()));
    put("gpu_mem_clock", json!(dev.clock_info(Clock::Memory).ok()));
    put("gpu_power", json!(dev.power_usage().ok().map(|mw| (mw as f64 / 1000.0).round() as u32)));
    put("gpu_pstate", json!(dev.performance_state().ok().map(pstate_name)));
    put("gpu_util", json!(dev.utilization_rates().ok().map(|u| u.gpu)));
    if let Ok(mem) = dev.memory_info() {
        put("vram_used", json!(mem.used / 1_048_576));
        put("vram_total", json!(mem.total / 1_048_576));
    }
    Some(m)
}

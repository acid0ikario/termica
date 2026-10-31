//! termicad: aplica perfiles térmicos (GPU NVIDIA, CPU Intel, ventiladores MSI).
//!
//! Corre como root y expone `com.ikario.Termica1` en el bus de sistema.
//! Con `--dev` usa el bus de sesión, no pide permisos y no toca el hardware.

mod hw;
mod service;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use service::{Config, State, Termica, apply_active, load_store};
use termica_core::{BUS_NAME, OBJECT_PATH};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // proceso hijo para usar NVML (ver hw::nvml_main)
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--nvml") {
        std::process::exit(hw::nvml_main(&args[1..]));
    }
    run()
}

#[tokio::main]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .format_target(false)
        .init();

    let dev = std::env::args().any(|a| a == "--dev");
    let conf_dir = std::env::var_os("TERMICA_CONF_DIR").map(PathBuf::from).unwrap_or_else(|| "/etc/termica".into());
    let cfg = Arc::new(Config { dev, conf_dir });
    let state = Arc::new(Mutex::new(State::new(load_store(&cfg))));

    let builder = if dev { zbus::connection::Builder::session() } else { zbus::connection::Builder::system() };
    let conn = builder
        .name(BUS_NAME)
        .serve_at(OBJECT_PATH, Termica { state: state.clone(), cfg: cfg.clone() })
        .build()
        .await?;
    log::info!("termicad listo{}", if dev { " (modo desarrollo)" } else { "" });

    apply_active(&state, &cfg, &hw::ALL).await;

    // La NVIDIA olvida los límites al dormir o cuando ningún programa la usa (por ejemplo al
    // cerrar sesión). Se reaplican al despertar y, cada 30 s, si la velocidad pasa del límite.
    {
        let (state, cfg) = (state.clone(), cfg.clone());
        tokio::spawn(async move {
            let mut was_awake = hw::gpu_awake();
            let mut tick = 0u32;
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                tick += 1;
                let awake = hw::gpu_awake();
                if awake && !was_awake {
                    log::info!("La GPU despertó: reaplico sus límites");
                    apply_active(&state, &cfg, &[hw::Part::Gpu]).await;
                } else if awake && tick.is_multiple_of(6) {
                    let gpu = state.lock().unwrap().store.active_profile().gpu.clone();
                    let over = tokio::task::spawn_blocking(move || hw::gpu_over_limit(&gpu)).await.unwrap_or(false);
                    if over {
                        log::info!("La GPU pasó su límite (se perdió): lo reaplico");
                        apply_active(&state, &cfg, &[hw::Part::Gpu]).await;
                    }
                }
                was_awake = awake;
            }
        });
    }

    // Cooler Boost automático: se enciende al llegar a `boost_at` y se apaga `boost_secs` después de
    // bajar de esa temperatura, para que el EC no llegue a su último tramo (que mantiene los
    // ventiladores altos hasta que todo se enfría mucho).
    {
        let (state, cfg) = (state.clone(), cfg.clone());
        tokio::spawn(async move {
            let set = |on: bool| {
                if cfg.dev {
                    log::info!("[dev] cooler_boost = {on}");
                } else if let Err(e) = hw::set_cooler_boost(on) {
                    log::warn!("Cooler Boost automático: {e}");
                }
            };
            let mut until: Option<Instant> = None;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let (at, secs, manual) = {
                    let s = state.lock().unwrap();
                    let f = &s.store.active_profile().fans;
                    (f.boost_at, f.boost_secs, f.cooler_boost || s.overrides.get("cooler_boost") == Some(&true))
                };
                let now = Instant::now();
                let temp = hw::hottest();
                match at {
                    // encendido a mano o desactivado: no se toca (el perfil ya dejó el boost como va)
                    _ if manual => until = None,
                    None => until = None,
                    Some(at) if temp.is_some_and(|t| t >= at as i64) => {
                        if until.is_none() {
                            log::info!("{}° ≥ {at}°: enciendo el Cooler Boost", temp.unwrap_or(0));
                        }
                        // se vuelve a escribir por si algo lo apagó (cambio de perfil, suspensión)
                        if !hw::cooler_boost_on() {
                            set(true);
                        }
                        until = Some(now + Duration::from_secs(secs as u64));
                    }
                    Some(_) => {
                        if until.is_some_and(|u| now >= u) {
                            log::info!("{}°: apago el Cooler Boost", temp.unwrap_or(0));
                            set(false);
                            until = None;
                        }
                    }
                }
                state.lock().unwrap().auto_boost = until.is_some();
            }
        });
    }

    // al volver de suspensión el EC y la CPU vuelven a sus valores: se reaplica todo
    if !dev {
        let (state, cfg, conn) = (state.clone(), cfg.clone(), conn.clone());
        tokio::spawn(async move {
            let Ok(login) = zbus::Proxy::new(
                &conn,
                "org.freedesktop.login1",
                "/org/freedesktop/login1",
                "org.freedesktop.login1.Manager",
            )
            .await
            else {
                return log::warn!("No pude conectar con logind");
            };
            let Ok(mut stream) = login.receive_signal("PrepareForSleep").await else {
                return log::warn!("No pude escuchar las suspensiones");
            };
            while let Some(msg) = stream.next().await {
                if let Ok(false) = msg.body().deserialize::<bool>() {
                    log::info!("Regreso de suspensión: reaplico el perfil");
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    apply_active(&state, &cfg, &hw::ALL).await;
                }
            }
        });
    }

    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
    log::info!("termicad se detiene");
    Ok(())
}

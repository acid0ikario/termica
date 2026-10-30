//! termicad: aplica perfiles térmicos (GPU NVIDIA, CPU Intel, ventiladores MSI).
//!
//! Corre como root y expone `com.ikario.Termica1` en el bus de sistema.
//! Con `--dev` usa el bus de sesión, no pide permisos y no toca el hardware.

mod hw;
mod service;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use service::{Config, State, Termica, apply_active, load_store};
use termica_core::{BUS_NAME, OBJECT_PATH};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

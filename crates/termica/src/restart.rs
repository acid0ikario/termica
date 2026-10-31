//! Restart the system service before replacing the desktop app process.
use std::ffi::OsStr;

use gtk::gio;

use crate::i18n::tr;

pub async fn service() -> Result<(), String> {
    // Development instances use a session-bus daemon, never the hardware service.
    if std::env::var_os("TERMICA_DEV").is_some() {
        return Ok(());
    }
    run(&[OsStr::new("systemctl"), OsStr::new("restart"), OsStr::new("termicad.service")])
        .await
        .map_err(|error| trf!("Could not restart termicad: {error}", "No se pudo reiniciar termicad: {error}"))
}

async fn run(args: &[&OsStr]) -> Result<(), String> {
    let process = gio::Subprocess::newv(args, gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_PIPE)
        .map_err(|error| error.to_string())?;
    let (_, stderr) = process.communicate_utf8_future(None).await.map_err(|error| error.to_string())?;
    if process.is_successful() {
        Ok(())
    } else {
        let detail = stderr.as_deref().unwrap_or_default().trim();
        Err(if detail.is_empty() { tr("The service restart command failed.").to_string() } else { detail.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::glib;

    fn command(args: &[&str]) -> Result<(), String> {
        let context = glib::MainContext::new();
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        context.with_thread_default(|| context.block_on(run(&args))).unwrap()
    }

    #[test]
    fn waits_for_successful_completion() {
        // Output alone must not be treated as failure; the exit status decides.
        assert!(command(&["/bin/sh", "-c", "printf 'notice' >&2; exit 0"]).is_ok());
    }

    #[test]
    fn reports_command_failure() {
        assert_eq!(command(&["/bin/sh", "-c", "printf 'Permission denied' >&2; exit 1"]), Err("Permission denied".into()));
        assert!(command(&["/bin/sh", "-c", "exit 1"]).is_err());
    }

    #[test]
    fn reports_missing_command() {
        assert!(command(&["/nonexistent/termica-restart-test"]).is_err());
    }
}

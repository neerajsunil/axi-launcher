//! Background self-update: checks GitHub for a newer release, downloads it, and installs it
//! when the launcher quits (or right away from the "Restart now" banner).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axi_core::update;
use slint::ComponentHandle;

use crate::state::AppState;
use crate::{AppApi, MainWindow};

/// How often a running launcher looks for a new release.
const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Set while the checking thread runs, so turning the setting on twice starts it once.
static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    update::clean_up();
    *state.update.lock().unwrap() = update::pending(&state.paths.updates_dir(), CURRENT);
    state.mark_dirty();
    start(state);

    let state = state.clone();
    let weak = ui.as_weak();
    ui.global::<AppApi>().on_restart_to_update(move || {
        if state.sessions().values().any(|s| s.task.is_some() || s.game.is_some()) {
            return state.error("Close your games first");
        }
        let Some(pending) = state.update.lock().unwrap().clone() else { return };
        if cfg!(debug_assertions) {
            return state.info("Debug builds don't install updates");
        }
        // Read before installing: the running exe gets moved aside on Windows.
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(e) => return state.error(format!("Couldn't update: {e}")),
        };
        if let Err(e) = update::install(&state.paths.updates_dir(), &pending) {
            return state.error(format!("Couldn't update: {e}"));
        }
        *state.update.lock().unwrap() = None;
        if let Err(e) = std::process::Command::new(&exe).spawn() {
            return state.error(format!("Updated to {}, but could not restart: {e}", pending.version));
        }
        if let Some(ui) = weak.upgrade() {
            let _ = ui.hide();
        }
        let _ = slint::quit_event_loop();
    });
}

/// Starts checking for updates in the background, unless turned off or already running.
/// Debug builds never update themselves (they still show a downloaded update, for testing the UI).
pub fn start(state: &Arc<AppState>) {
    if cfg!(debug_assertions) || !update::SUPPORTED || !state.settings().auto_update || RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let state = state.clone();
    std::thread::spawn(move || {
        loop {
            if !state.settings().auto_update {
                break;
            }
            if let Err(e) = check(&state) {
                eprintln!("update check failed: {e}");
            }
            std::thread::sleep(INTERVAL);
        }
        RUNNING.store(false, Ordering::Release);
    });
}

/// Downloads the latest release unless it is already waiting to be installed.
fn check(state: &AppState) -> axi_core::Result<()> {
    let Some(release) = update::check(&state.client, CURRENT)? else { return Ok(()) };
    if state.update.lock().unwrap().as_ref().is_some_and(|p| !update::is_newer(&release.version, &p.version)) {
        return Ok(());
    }
    let pending = update::download(&state.client, &release, &state.paths.updates_dir())?;
    *state.update.lock().unwrap() = Some(pending);
    state.mark_dirty();
    Ok(())
}

/// Installs a downloaded update as the launcher quits, so the next start runs it.
pub fn install_on_exit(state: &AppState) {
    if cfg!(debug_assertions) {
        return;
    }
    let Some(pending) = state.update.lock().unwrap().take() else { return };
    if let Err(e) = update::install(&state.paths.updates_dir(), &pending) {
        eprintln!("could not install update {}: {e}", pending.version);
    }
}

//! UI callbacks, one module per area.

mod accounts;
mod browse;
mod instance_settings;
mod instances;
mod java;
mod lan_sync;
mod launch;
mod loaders;
mod modpacks;
mod mods;
mod settings;
pub mod update;
mod versions;

use std::sync::Arc;

use crate::MainWindow;
use crate::state::AppState;

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    settings::register(ui, state);
    java::register(ui, state);
    accounts::register(ui, state);
    versions::register(ui, state);
    instances::register(ui, state);
    instance_settings::register(ui, state);
    launch::register(ui, state);
    lan_sync::register(ui, state);
    loaders::register(ui, state);
    mods::register(ui, state);
    browse::register(ui, state);
    modpacks::register(ui, state);
    update::register(ui, state);
}

/// Opens a file or folder with the system's default app.
fn open_path(state: &AppState, path: &std::path::Path) {
    if let Err(e) = open::that_detached(path) {
        state.error(format!("Could not open {}: {e}", path.display()));
    }
}

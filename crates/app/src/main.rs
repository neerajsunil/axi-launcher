#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod controllers;
mod convert;
mod format;
mod heads;
mod icons;
mod instance_icons;
mod lan;
mod state;
mod sync;

use std::sync::Arc;

use axi_core::paths::Paths;
use slint::ComponentHandle;

use state::AppState;

slint::include_modules!();

fn main() -> anyhow::Result<()> {
    let state = Arc::new(AppState::load(Paths::new(Paths::default_root()))?);
    lan::start(&state);
    let ui = MainWindow::new()?;
    controllers::register(&ui, &state);
    let _sync = sync::start(&ui, state.clone());

    // The window can be hidden while a game runs, so quit explicitly when it is closed.
    ui.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    ui.show()?;
    slint::run_event_loop_until_quit()?;
    controllers::update::install_on_exit(&state);
    Ok(())
}

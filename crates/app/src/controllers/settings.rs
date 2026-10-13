//! The global settings page.

use std::sync::Arc;

use axi_core::settings::Theme as ThemeSetting;
use slint::ComponentHandle;

use super::open_path;
use crate::convert;
use crate::state::AppState;
use crate::{AppApi, InstancesApi, MainWindow, SettingsApi, Theme};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    ui.global::<AppApi>().set_version(env!("CARGO_PKG_VERSION").into());
    ui.global::<SettingsApi>().set_built_in_curseforge_key(axi_core::settings::BUILT_IN_CURSEFORGE_KEY.is_some_and(|k| !k.trim().is_empty()));
    ui.global::<SettingsApi>().set_built_in_client_id(axi_core::account::microsoft::BUILT_IN_CLIENT_ID.is_some_and(|id| !id.is_empty()));
    show(ui, state);

    let api = ui.global::<SettingsApi>();
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_save(move |data| {
            let Some(ui) = weak.upgrade() else { return };
            let mut settings = state.settings().clone();
            if let Err(e) = convert::apply_global_settings(&data, &mut settings) {
                return state.error(e);
            }
            if let Err(e) = settings.save(&state.paths.settings_file()) {
                return state.error(format!("Could not save settings: {e}"));
            }
            *state.settings() = settings;
            show(&ui, &state);
            state.info("Settings saved");
        });
    }
    {
        let state = state.clone();
        api.on_open_data_dir(move || open_path(&state, state.paths.root()));
    }
}

/// Pushes the current settings into every place the UI shows them.
fn show(ui: &MainWindow, state: &AppState) {
    let settings = state.settings().clone();
    ui.global::<SettingsApi>().set_current(convert::global_settings(&settings, &state.paths));
    ui.global::<Theme>().set_mode(match settings.theme {
        ThemeSetting::System => 0,
        ThemeSetting::Light => 1,
        ThemeSetting::Dark => 2,
    });

    // Instance settings show global values where they aren't overridden.
    let api = ui.global::<InstancesApi>();
    if let Some(instance) = state.instance(&api.get_selected_id()) {
        api.set_settings(convert::instance_settings(&instance, &settings));
    }
}

//! Creating, copying, renaming and deleting instances.

use std::sync::Arc;
use std::thread;

use axi_core::instance::{GameSetup, unique_name};
use slint::ComponentHandle;

use super::{loaders, open_path};
use crate::{convert, instance_icons, sync};
use crate::state::AppState;
use crate::{InstancesApi, MainWindow};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<InstancesApi>();

    {
        let state = state.clone();
        api.on_search_changed(move |text| {
            *state.search.lock().unwrap() = text.to_string();
            state.mark_dirty();
        });
    }
    {
        let state = state.clone();
        api.on_sort_changed(move || {
            state.mark_dirty();
            sync::flush();
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_select(move |id| {
            if let Some(ui) = weak.upgrade() {
                select(&ui, &state, &id);
            }
        });
    }
    {
        let state = state.clone();
        api.on_create(move |name, group, version, loader, loader_version| {
            let name = if name.trim().is_empty() { version.to_string() } else { name.trim().to_string() };
            let name = unique_name(&state.manager.names(), &name);
            let loader = convert::loader_from_index(loader);
            let game = GameSetup {
                loader_version: loaders::resolve(&state, loader, &version, &loader_version),
                version: version.to_string(),
                loader,
            };
            match state.manager.create(&name, &group, game) {
                Ok(_) => state.info(format!("\"{name}\" is ready to play")),
                Err(e) => state.error(format!("Couldn't create instance: {e}")),
            }
            state.reload_instances();
        });
    }
    {
        let state = state.clone();
        api.on_delete(move |id| {
            if state.is_active(&id) {
                return state.error("Close the game first");
            }
            let name = state.instance(&id).map(|i| i.config.name).unwrap_or_default();
            match state.manager.delete(&id) {
                Ok(()) => state.info(format!("Deleted \"{name}\"")),
                Err(e) => { eprintln!("could not delete instance: {e}"); state.error("Couldn't delete it") },
            }
            state.sessions().remove(id.as_str());
            state.reload_instances();
        });
    }
    {
        let state = state.clone();
        api.on_copy(move |id, name| {
            let name = unique_name(&state.manager.names(), &name);
            state.info("Making a copy...");
            let state = state.clone();
            thread::spawn(move || {
                match state.manager.copy(&id, &name) {
                    Ok(_) => state.info(format!("\"{name}\" is ready to play")),
                    Err(e) => { eprintln!("could not copy instance: {e}"); state.error("Couldn't copy it") },
                }
                state.reload_instances();
            });
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_rename(move |id, name| {
            if let Err(e) = state.manager.rename(&id, &name) {
                state.error(format!("Couldn't rename instance: {e}"));
            }
            state.reload_instances();
            if let Some(ui) = weak.upgrade() {
                select(&ui, &state, &id);
            }
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_change_game(move |id, version, loader, loader_version| {
            if state.is_active(&id) {
                return state.error("Close the game first");
            }
            let loader = convert::loader_from_index(loader);
            let loader_version = loaders::resolve(&state, loader, &version, &loader_version);
            let result = state.manager.get(&id).and_then(|mut instance| {
                let c = &mut instance.config;
                c.game_version = version.to_string();
                c.loader = loader;
                c.loader_version = loader_version.clone();
                instance.save()
            });
            let label = format!("{version} {} {loader_version}", loader.display_name());
            match result {
                Ok(()) => state.info(format!("Switched to {}", label.trim())),
                Err(e) => state.error(format!("Couldn't change version: {e}")),
            }
            state.reload_instances();
            if let Some(ui) = weak.upgrade() {
                select(&ui, &state, &id);
            }
        });
    }
    {
        let state = state.clone();
        api.on_open_folder(move |id| {
            if let Some(instance) = state.instance(&id) {
                let dir = instance.game_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_path(&state, &dir);
            }
        });
    }
    {
        let state = state.clone();
        api.on_change_icon(move |id| {
            let Some(instance) = state.instance(&id) else { return };
            let file = rfd::FileDialog::new()
                .set_title("Choose a picture")
                .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                .pick_file();
            let Some(path) = file else { return };
            match instance_icons::set(&instance, &path) {
                Ok(()) => state.mark_dirty(),
                Err(e) => { eprintln!("could not use that picture: {e}"); state.error("Couldn't use that picture") },
            }
        });
    }
    {
        let state = state.clone();
        api.on_reset_icon(move |id| {
            let Some(instance) = state.instance(&id) else { return };
            match instance_icons::reset(&state, &instance) {
                Ok(()) => state.mark_dirty(),
                Err(e) => { eprintln!("could not remove the picture: {e}"); state.error("Couldn't remove the picture") },
            }
        });
    }
    {
        let state = state.clone();
        api.on_open_log(move |id| match state.instance(&id).map(|i| i.log_file()) {
            Some(log) if log.exists() => open_path(&state, &log),
            _ => state.error("No logs yet. Go play!"),
        });
    }
    {
        let state = state.clone();
        api.on_clear_logs(move |id| state.clear_logs(&id));
    }
}

/// Makes `id` the selected instance and loads its settings form.
pub fn select(ui: &MainWindow, state: &AppState, id: &str) {
    let api = ui.global::<InstancesApi>();
    api.set_selected_id(id.into());
    if let Some(instance) = state.instance(id) {
        api.set_settings(convert::instance_settings(&instance, &state.settings()));
    }
    state.mark_dirty();
    sync::flush();
}

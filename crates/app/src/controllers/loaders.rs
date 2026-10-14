//! Loader versions for the loader picker.

use std::sync::Arc;
use std::thread;

use axi_core::loader::{self, ModLoader};
use slint::ComponentHandle;

use crate::convert;
use crate::state::AppState;
use crate::{LoadersApi, MainWindow};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let state = state.clone();
    ui.global::<LoadersApi>().on_query(move |index, game_version| {
        query(&state, convert::loader_from_index(index), &game_version);
    });
}

fn key(loader: ModLoader, game_version: &str) -> String {
    format!("{}|{game_version}", convert::loader_index(loader))
}

fn query(state: &Arc<AppState>, loader: ModLoader, game_version: &str) {
    let key = key(loader, game_version);
    let generation = {
        let mut list = state.loaders.lock().unwrap();
        list.key = key.clone();
        list.error.clear();
        list.generation += 1;
        list.loading = !list.cache.contains_key(&key);
        if !list.loading {
            drop(list);
            state.mark_dirty();
            return;
        }
        list.generation
    };
    state.mark_dirty();

    let (state, game_version) = (state.clone(), game_version.to_string());
    thread::spawn(move || {
        let result = loader::fetch_versions(&state.client, loader, &game_version);
        let mut list = state.loaders.lock().unwrap();
        if list.generation != generation {
            return; // a newer query replaced this one
        }
        list.loading = false;
        match result {
            Ok(versions) => {
                list.cache.insert(key, versions);
            }
            Err(e) => list.error = format!("Couldn't load {} versions: {e}", loader.display_name()),
        }
        drop(list);
        state.mark_dirty();
    });
}

/// The loader version to save: the user's choice, or the recommended one for "".
/// Falls back to "" (resolved at launch) when the list isn't loaded.
pub fn resolve(state: &AppState, loader: ModLoader, game_version: &str, chosen: &str) -> String {
    if !loader.is_modded() {
        return String::new();
    }
    if !chosen.is_empty() {
        return chosen.to_string();
    }
    let list = state.loaders.lock().unwrap();
    list.cache
        .get(&key(loader, game_version))
        .and_then(|versions| loader::recommended(versions))
        .map(|v| v.version.clone())
        .unwrap_or_default()
}

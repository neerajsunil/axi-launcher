//! The game version list shown in the version pickers.

use std::sync::{Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::convert;
use crate::state::AppState;
use crate::{MainWindow, VersionItem, VersionsApi};

/// The picker's current filter: (search text, include snapshots).
type Query = Arc<Mutex<(String, bool)>>;

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let query: Query = Arc::default();
    let api = ui.global::<VersionsApi>();

    {
        let (weak, state, query) = (ui.as_weak(), state.clone(), query.clone());
        api.on_query(move |search, snapshots| {
            *query.lock().unwrap() = (search.to_string(), snapshots);
            if let Some(ui) = weak.upgrade() {
                apply_query(&ui, &state, &query);
            }
        });
    }
    {
        let (weak, state, query) = (ui.as_weak(), state.clone(), query.clone());
        api.on_reload(move || load(weak.clone(), state.clone(), query.clone()));
    }
    load(ui.as_weak(), state.clone(), query);
}

fn load(weak: slint::Weak<MainWindow>, state: Arc<AppState>, query: Query) {
    state.versions.lock().unwrap().loading = true;
    state.mark_dirty();
    thread::spawn(move || {
        let cache = state.paths.cache_dir().join("version_manifest.json");
        let result = axi_core::meta::fetch_versions(&state.client, &cache);
        {
            let mut list = state.versions.lock().unwrap();
            list.loading = false;
            match result {
                Ok(entries) => {
                    list.entries = entries;
                    list.error.clear();
                }
                Err(e) => list.error = format!("Couldn't load versions: {e}"),
            }
        }
        state.mark_dirty();
        let _ = weak.upgrade_in_event_loop(move |ui| apply_query(&ui, &state, &query));
    });
}

fn apply_query(ui: &MainWindow, state: &AppState, query: &Query) {
    let (search, snapshots) = query.lock().unwrap().clone();
    let search = search.trim().to_lowercase();
    let items: Vec<VersionItem> = state
        .versions
        .lock()
        .unwrap()
        .entries
        .iter()
        .filter(|v| (snapshots || v.is_release()) && v.id.to_lowercase().contains(&search))
        .map(convert::version_item)
        .collect();
    ui.global::<VersionsApi>().set_items(ModelRc::new(VecModel::from(items)));
}

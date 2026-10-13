//! Installing modpacks as new instances, from Modrinth / CurseForge or a file on disk.
//! Installs run in the background one at a time; progress shows on the new instance's card.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use axi_core::instance::{PackOrigin, unique_name};
use axi_core::modpack::{self, SkippedFile};
use axi_core::mods::{ModApi, ModSource};
use slint::ComponentHandle;

use crate::convert;
use crate::state::AppState;
use crate::{MainWindow, ModpacksApi};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<ModpacksApi>();
    {
        let state = state.clone();
        api.on_install(move |source, project_id| {
            let source = convert::source_from_index(source);
            let project_id = project_id.to_string();
            let title = {
                let mut browse = state.browse.lock().unwrap();
                if !browse.installing.insert(project_id.clone()) {
                    return;
                }
                browse.hits.iter().find(|h| h.project_id == project_id).map_or("The modpack".into(), |h| h.title.clone())
            };
            announce(&state, &title);
            let state = state.clone();
            thread::spawn(move || {
                let _guard = state.packs_lock.lock().unwrap();
                let downloaded = download(&state, source, &project_id);
                match downloaded {
                    Ok((path, origin)) => {
                        install(&state, &path, Some(origin));
                        let _ = std::fs::remove_file(path);
                    }
                    Err(e) => state.error(format!("Could not download {title}: {e}")),
                }
                state.browse.lock().unwrap().installing.remove(&project_id);
                state.mark_dirty();
            });
        });
    }
    {
        let state = state.clone();
        api.on_import_file(move || {
            let file = rfd::FileDialog::new()
                .set_title("Import a modpack")
                .add_filter("Modpacks (.mrpack, .zip)", &["mrpack", "zip"])
                .pick_file();
            let Some(path) = file else { return };
            announce(&state, &path.file_name().unwrap_or_default().to_string_lossy());
            let state = state.clone();
            thread::spawn(move || {
                let _guard = state.packs_lock.lock().unwrap();
                install(&state, &path, None);
            });
        });
    }
}

fn announce(state: &AppState, title: &str) {
    let busy = state.packs_lock.try_lock().is_err();
    state.info(if busy { format!("{title} queued for download") } else { format!("Downloading {title}...") });
}

fn download(state: &AppState, source: ModSource, project_id: &str) -> axi_core::Result<(PathBuf, PackOrigin)> {
    let key = state.settings().curseforge_key();
    let api = ModApi { client: &state.client, curseforge_key: &key };
    let dir = state.paths.cache_dir().join("modpacks");
    modpack::download(&api, source, project_id, &dir, &|_, _| {})
}

/// Creates an instance from the pack at `path` and fills it. Reports the outcome itself.
fn install(state: &Arc<AppState>, path: &Path, origin: Option<PackOrigin>) {
    let pack = match modpack::open(path) {
        Ok(pack) => pack,
        Err(e) => return state.error(format!("Could not open the modpack: {e}")),
    };
    let origin = origin.unwrap_or_else(|| PackOrigin {
        name: pack.name.clone(),
        version: pack.version.clone(),
        ..Default::default()
    });
    let name = unique_name(&state.manager.names(), &origin.name);
    let mut instance = match state.manager.create(&name, "", pack.game.clone()) {
        Ok(instance) => instance,
        Err(e) => return state.error(format!("Could not create the instance: {e}")),
    };
    state.reload_instances();

    // Progress shows on the instance itself, which can't be played meanwhile.
    let id = instance.id.clone();
    let progress = |text: String, p: f32| state.set_task(&id, Some((text, p)));
    progress(format!("Installing {}...", origin.name), 0.0);
    let key = state.settings().curseforge_key();
    let threads = state.settings().concurrent_downloads;
    let api = ModApi { client: &state.client, curseforge_key: &key };
    let result = modpack::install(&api, &pack, &instance, threads, &progress);
    state.set_task(&id, None);

    match result {
        Ok(skipped) => {
            instance.config.pack = Some(origin.clone());
            if !skipped.is_empty() {
                instance.config.notes = manual_downloads_note(&skipped);
            }
            match instance.save() {
                Ok(()) if skipped.is_empty() => state.info(format!("Installed {}", origin.name)),
                Ok(()) => state.info(format!(
                    "Installed {}, but {} files need a manual download. See the instance notes.",
                    origin.name,
                    skipped.len()
                )),
                Err(e) => state.error(format!("Could not save the instance: {e}")),
            }
        }
        Err(e) => {
            let _ = state.manager.delete(&id);
            state.error(format!("Could not install {}: {e}", origin.name));
        }
    }
    state.reload_instances();
}

fn manual_downloads_note(skipped: &[SkippedFile]) -> String {
    let mut note = String::from(
        "The authors of these files only allow downloads from the CurseForge website. \
         Download them there and put them in the matching folder (mods, resourcepacks or shaderpacks):\n",
    );
    for file in skipped {
        note.push_str(&format!("\n- {}", file.title));
        if !file.page_url.is_empty() {
            note.push_str(&format!(" ({})", file.page_url));
        }
    }
    note
}

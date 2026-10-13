//! The Content tab: installed mods, resource packs and shaders; enabling, removing,
//! updating them and picking a specific version. Installs run in the background, one
//! batch after another (each batch downloads in parallel), and report through toasts.

use std::sync::Arc;
use std::thread;

use axi_core::mods::{self, ContentKind, ModApi, ModSource};
use slint::ComponentHandle;

use super::open_path;
use crate::convert::kind_from_index;
use crate::state::AppState;
use crate::sync;
use crate::{MainWindow, ModsApi};

pub use axi_core::mods::InstallRequest as Job;

/// Rows the version picker adds each time it is scrolled to the end.
const VERSION_PAGE: usize = 30;

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<ModsApi>();
    {
        let state = state.clone();
        api.on_refresh(move |id, kind| {
            refresh(&state, &id, kind_from_index(kind));
            sync::flush();
        });
    }
    {
        let state = state.clone();
        api.on_filter(move |text| {
            state.mods.lock().unwrap().filter = text.to_string();
            state.mark_dirty();
            sync::flush();
        });
    }
    {
        let state = state.clone();
        api.on_set_enabled(move |id, kind, file, enabled| {
            let Some(instance) = state.instance(&id) else { return };
            let kind = kind_from_index(kind);
            if let Err(e) = mods::set_enabled(&instance, kind, &file, enabled) {
                state.error(format!("Could not change {file}: {e}. Close the game and try again."));
            }
            refresh(&state, &id, kind);
        });
    }
    {
        let state = state.clone();
        api.on_remove(move |id, kind, file| {
            let Some(instance) = state.instance(&id) else { return };
            let kind = kind_from_index(kind);
            let Ok(_guard) = state.mods_lock.try_lock() else {
                return state.error("Wait for the downloads to finish");
            };
            match mods::remove(&instance, kind, &file) {
                Ok(()) => state.info(format!("Removed {file}")),
                Err(e) => state.error(format!("Could not remove {file}: {e}. Close the game and try again.")),
            }
            state.mods.lock().unwrap().updates.remove(file.as_str());
            refresh(&state, &id, kind);
        });
    }
    {
        let state = state.clone();
        api.on_open_folder(move |id, kind| {
            if let Some(instance) = state.instance(&id) {
                let dir = instance.content_dir(kind_from_index(kind));
                let _ = std::fs::create_dir_all(&dir);
                open_path(&state, &dir);
            }
        });
    }
    {
        let state = state.clone();
        api.on_install_shader_loader(move |id| {
            let iris = Job::newest(ModSource::Modrinth, mods::IRIS_MODRINTH_ID);
            install(state.clone(), id.to_string(), ContentKind::Mod, vec![iris], "Iris".into());
        });
    }
    {
        let state = state.clone();
        api.on_check_updates(move |id, kind| check_updates(&state, &id, kind_from_index(kind)));
    }
    {
        let state = state.clone();
        api.on_update(move |id, kind, file| {
            let found = state.mods.lock().unwrap().items.iter().find(|m| m.file_name == file.as_str()).and_then(|m| {
                let entry = m.entry.as_ref()?;
                Some((Job::newest(entry.source, entry.project_id.clone()), m.title.clone()))
            });
            if let Some((job, title)) = found {
                install(state.clone(), id.to_string(), kind_from_index(kind), vec![job], title);
            }
        });
    }
    {
        let state = state.clone();
        api.on_update_all(move |id, kind| {
            let jobs: Vec<_> = {
                let view = state.mods.lock().unwrap();
                view.items
                    .iter()
                    .filter(|m| view.updates.contains(&m.file_name))
                    .filter_map(|m| m.entry.as_ref().map(|e| Job::newest(e.source, e.project_id.clone())))
                    .collect()
            };
            let label = if jobs.len() == 1 { "1 update".to_string() } else { format!("{} updates", jobs.len()) };
            install(state.clone(), id.to_string(), kind_from_index(kind), jobs, label);
        });
    }
    {
        let state = state.clone();
        api.on_open_versions(move |id, kind, file| {
            open_versions(&state, &id, kind_from_index(kind), &file);
            sync::flush();
        });
    }
    {
        let state = state.clone();
        api.on_show_incompatible_changed(move |show| {
            let mut picker = state.picker.lock().unwrap();
            picker.show_incompatible = show;
            picker.limit = VERSION_PAGE;
            drop(picker);
            state.mark_dirty();
        });
    }
    {
        let state = state.clone();
        // Scrolling fires this repeatedly; grow once per batch the dialog has shown.
        api.on_versions_load_more(move |shown| {
            let mut picker = state.picker.lock().unwrap();
            if picker.limit <= shown as usize {
                picker.limit += VERSION_PAGE;
                drop(picker);
                state.mark_dirty();
            }
        });
    }
    {
        let state = state.clone();
        api.on_install_version(move |file_id| {
            let found = {
                let picker = state.picker.lock().unwrap();
                picker.items.iter().find(|v| v.file_id == file_id.as_str()).map(|v| {
                    let job = Job {
                        source: picker.source,
                        project_id: picker.project_id.clone(),
                        file_id: Some(v.file_id.clone()),
                    };
                    (picker.instance.clone(), picker.kind, job, format!("{} {}", picker.title, v.version_number))
                })
            };
            if let Some((id, kind, job, label)) = found {
                install(state.clone(), id, kind, vec![job], label);
            }
        });
    }
}

/// Reloads the installed `kind` content of instance `id` in the background.
pub fn refresh(state: &Arc<AppState>, id: &str, kind: ContentKind) {
    {
        let mut view = state.mods.lock().unwrap();
        if view.instance != id || view.kind != kind {
            view.instance = id.to_string();
            view.kind = kind;
            view.items.clear();
            view.updates.clear();
        }
        view.loading = true;
    }
    state.mark_dirty();
    let (state, id) = (state.clone(), id.to_string());
    thread::spawn(move || {
        let Some(instance) = state.instance(&id) else { return };
        let result = mods::list_installed(&instance, kind);
        let shader_loader = kind == ContentKind::Shader && mods::has_shader_loader(&instance);
        let mut view = state.mods.lock().unwrap();
        if view.instance != id || view.kind != kind {
            return;
        }
        view.loading = false;
        view.shader_loader = shader_loader;
        match result {
            Ok(items) => view.items = items,
            Err(e) => {
                drop(view);
                state.error(format!("Could not read the {} folder: {e}", kind.folder()));
            }
        }
        state.mark_dirty();
    });
}

fn check_updates(state: &Arc<AppState>, id: &str, kind: ContentKind) {
    let Some(instance) = state.instance(id) else { return };
    state.mods.lock().unwrap().checking = true;
    state.mark_dirty();
    let state = state.clone();
    thread::spawn(move || {
        let key = state.settings().curseforge_key();
        let api = ModApi { client: &state.client, curseforge_key: &key };
        let threads = state.settings().concurrent_downloads;
        let result = mods::check_updates(&api, &instance, kind, threads);
        let mut view = state.mods.lock().unwrap();
        view.checking = false;
        let message = match result {
            Ok(updates) if view.instance == instance.id && view.kind == kind => {
                let n = updates.len();
                view.updates = updates.into_iter().collect();
                Ok(match n {
                    0 => format!("All {} are up to date", kind.plural()),
                    1 => "1 update available".to_string(),
                    n => format!("{n} updates available"),
                })
            }
            Ok(_) => Ok(String::new()),
            Err(e) => Err(format!("Could not check for updates: {e}")),
        };
        drop(view);
        match message {
            Ok(text) if !text.is_empty() => state.info(text),
            Ok(_) => state.mark_dirty(),
            Err(text) => state.error(text),
        }
    });
}

/// Loads every version of an installed project for the version picker.
fn open_versions(state: &Arc<AppState>, id: &str, kind: ContentKind, file_name: &str) {
    let Some(instance) = state.instance(id) else { return };
    let Some(item) = state.mods.lock().unwrap().items.iter().find(|m| m.file_name == file_name).cloned() else { return };
    let Some(entry) = item.entry else { return };
    let generation = {
        let mut picker = state.picker.lock().unwrap();
        picker.generation += 1;
        picker.instance = id.to_string();
        picker.kind = kind;
        picker.title = item.title;
        picker.source = entry.source;
        picker.project_id = entry.project_id.clone();
        picker.current = entry.file_id;
        picker.items.clear();
        picker.show_incompatible = false;
        picker.limit = VERSION_PAGE;
        picker.error.clear();
        picker.loading = true;
        picker.generation
    };
    state.mark_dirty();
    let state = state.clone();
    thread::spawn(move || {
        let key = state.settings().curseforge_key();
        let api = ModApi { client: &state.client, curseforge_key: &key };
        let config = &instance.config;
        let result = api.versions(entry.source, kind, &entry.project_id, &config.game_version, config.loader);
        let mut picker = state.picker.lock().unwrap();
        if picker.generation != generation {
            return;
        }
        picker.loading = false;
        match result {
            Ok(items) => picker.items = items,
            Err(e) => picker.error = format!("Could not load versions: {e}"),
        }
        drop(picker);
        state.mark_dirty();
    });
}

/// Queues `jobs` of `kind` (mods bring their dependencies) for instance `id`. Batches run
/// one after another in the background; `label` names them in the "queued" toast.
pub fn install(state: Arc<AppState>, id: String, kind: ContentKind, jobs: Vec<Job>, label: String) {
    // Skip projects that are already queued.
    let jobs: Vec<Job> = {
        let mut browse = state.browse.lock().unwrap();
        jobs.into_iter().filter(|j| browse.installing.insert(j.project_id.clone())).collect()
    };
    if jobs.is_empty() {
        return;
    }
    let busy = state.mods_lock.try_lock().is_err();
    state.info(if busy { format!("{label} queued for download") } else { format!("Downloading {label}...") });

    thread::spawn(move || {
        let _guard = state.mods_lock.lock().unwrap();
        let instance = state.instance(&id);
        let key = state.settings().curseforge_key();
        let api = ModApi { client: &state.client, curseforge_key: &key };

        let threads = state.settings().concurrent_downloads;
        let report = match &instance {
            Some(instance) => mods::install(&api, instance, kind, &jobs, threads, &|_, _| {}),
            None => Err(axi_core::Error::msg("The instance no longer exists")),
        };
        let (installed, failed) = match report {
            Ok(report) => (report.installed, report.failed),
            Err(e) => (Vec::new(), vec![e.to_string()]),
        };
        {
            let mut browse = state.browse.lock().unwrap();
            for job in &jobs {
                browse.installing.remove(&job.project_id);
            }
        }
        state.mark_dirty();

        let current = {
            let mut view = state.mods.lock().unwrap();
            // Updated files have new names; drop flags that no longer apply.
            if view.instance == id {
                view.updates.clear();
            }
            view.kind
        };
        if let Some(error) = failed.first() {
            state.error(format!("Could not install: {error}"));
        } else if !installed.is_empty() {
            state.info(match installed.as_slice() {
                [one] => format!("Installed {one}"),
                [first, rest @ ..] => format!("Installed {first} and {} more", rest.len()),
                [] => unreachable!(),
            });
        }
        // Installing Iris from the Shaders view changes what that view shows.
        refresh(&state, &id, current);
    });
}

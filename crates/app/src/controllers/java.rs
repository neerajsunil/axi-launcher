//! The Java picker: installed Java, downloads from several providers, and browsing for an executable.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use axi_core::java::{self, JavaProvider, JavaRelease};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::state::AppState;
use crate::{DialogKind, JavaApi, JavaInstallItem, JavaReleaseItem, MainWindow, Nav};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<JavaApi>();
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_detect(move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<JavaApi>().set_detecting(true);
            }
            let (weak, state) = (weak.clone(), state.clone());
            thread::spawn(move || detect(&weak, &state));
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_load_releases(move |index| {
            let Some(&provider) = JavaProvider::ALL.get(index as usize) else { return };
            let Some(ui) = weak.upgrade() else { return };
            let api = ui.global::<JavaApi>();
            api.set_releases(ModelRc::default());
            api.set_releases_error(SharedString::new());
            api.set_releases_loading(true);
            state.java_releases.lock().unwrap().clear();

            let (weak, state) = (weak.clone(), state.clone());
            thread::spawn(move || {
                let result = java::available_releases(&state.client, provider);
                let error = match result {
                    Ok(releases) => {
                        *state.java_releases.lock().unwrap() = releases;
                        String::new()
                    }
                    Err(e) => format!("Could not reach {}: {e}", provider.name()),
                };
                let items = release_items(&state);
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let api = ui.global::<JavaApi>();
                    // The user may have switched providers while this loaded.
                    if api.get_provider() != index {
                        return;
                    }
                    api.set_releases(ModelRc::new(VecModel::from(items)));
                    api.set_releases_error(error.into());
                    api.set_releases_loading(false);
                });
            });
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_install(move |provider, index| {
            let wanted = JavaProvider::ALL.get(provider as usize).copied();
            let release = state.java_releases.lock().unwrap().get(index as usize).cloned();
            let Some(release) = release.filter(|r| Some(r.provider) == wanted) else { return };
            {
                let mut task = state.java_task.lock().unwrap();
                if task.is_some() {
                    return state.error("Java is already downloading");
                }
                *task = Some((format!("Downloading {}...", release.name()), 0.0));
            }
            state.mark_dirty();
            let (weak, state) = (weak.clone(), state.clone());
            thread::spawn(move || install(&weak, &state, &release));
        });
    }
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_remove(move |path| {
            if state.java_task.lock().unwrap().is_some() {
                return state.error("Wait for the Java download to finish");
            }
            let path = PathBuf::from(path.as_str());
            match java::remove_install(&state.paths.runtime_dir(), &path) {
                Ok(()) => state.info("Java deleted"),
                Err(e) => return state.error(format!("Could not delete Java: {e}")),
            }
            let Some(ui) = weak.upgrade() else { return };
            let api = ui.global::<JavaApi>();
            // A page using it falls back to Automatic.
            if api.get_path().as_str() == path.to_string_lossy() {
                api.invoke_pick(SharedString::new());
            }
            api.set_releases(ModelRc::new(VecModel::from(release_items(&state))));
            let (weak, state) = (weak.clone(), state.clone());
            thread::spawn(move || detect(&weak, &state));
        });
    }
    {
        let weak = ui.as_weak();
        api.on_browse(move || {
            let mut dialog = rfd::FileDialog::new().set_title("Select a Java executable");
            if cfg!(windows) {
                dialog = dialog.add_filter("Java (java.exe, javaw.exe)", &["exe"]);
            }
            let Some(path) = dialog.pick_file() else { return };
            let Some(ui) = weak.upgrade() else { return };
            ui.global::<JavaApi>().invoke_pick(path.to_string_lossy().as_ref().into());
            ui.global::<Nav>().set_dialog(DialogKind::None);
        });
    }
}

fn install(weak: &Weak<MainWindow>, state: &AppState, release: &JavaRelease) {
    let progress = |text: String, value: f32| {
        *state.java_task.lock().unwrap() = Some((text, value));
        state.mark_dirty();
    };
    let threads = state.settings().concurrent_downloads;
    let result = java::install_release(&state.client, &state.paths.runtime_dir(), release, threads, &progress);
    *state.java_task.lock().unwrap() = None;
    match result {
        Ok(_) => state.info(format!("{} downloaded", release.name())),
        Err(e) => state.error(format!("Could not download {}: {e}", release.name())),
    }

    // Show it as installed in both tabs.
    let items = release_items(state);
    let provider = JavaProvider::ALL.iter().position(|&p| p == release.provider).unwrap_or(0) as i32;
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let api = ui.global::<JavaApi>();
        if api.get_provider() == provider {
            api.set_releases(ModelRc::new(VecModel::from(items)));
        }
    });
    detect(weak, state);
}

/// Rows for the Download tab from the releases last loaded.
fn release_items(state: &AppState) -> Vec<JavaReleaseItem> {
    let runtimes = state.paths.runtime_dir();
    state
        .java_releases
        .lock()
        .unwrap()
        .iter()
        .map(|release| {
            let installed = java::installed_release(&runtimes, release);
            let installed_version = installed.as_deref().and_then(java::installed_version).unwrap_or_default();
            let outdated = java::is_outdated(&installed_version, &release.version);
            let version = match (release.version.as_str(), outdated) {
                ("", _) => "Latest build".to_string(),
                (latest, true) => format!("{installed_version} installed, {latest} available"),
                (latest, false) => format!("Latest {latest}"),
            };
            let games = java::games_for_java(release.major);
            let detail = if games.is_empty() { version } else { format!("{version}  ·  {games}") };
            JavaReleaseItem {
                title: format!("Java {}", release.major).into(),
                detail: detail.into(),
                lts: release.is_lts(),
                newer: release.major > java::NEWEST_GAME_JAVA,
                installed: installed.is_some(),
                outdated,
                path: installed.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default().into(),
            }
        })
        .collect()
}

/// Scans for Java installs and fills the Installed tab.
fn detect(weak: &Weak<MainWindow>, state: &AppState) {
    let runtimes = state.paths.runtime_dir();
    let items: Vec<JavaInstallItem> = java::detect_installations(&runtimes)
        .into_iter()
        .map(|j| {
            let title = match (j.version.as_str(), j.major) {
                ("", 0) => "Unknown Java".to_string(),
                ("", major) => format!("Java {major}"),
                (version, _) => format!("Java {version}"),
            };
            JavaInstallItem {
                title: title.into(),
                vendor: j.vendor.as_str().into(),
                managed: j.path.starts_with(&runtimes),
                path: j.path.to_string_lossy().as_ref().into(),
            }
        })
        .collect();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        let api = ui.global::<JavaApi>();
        api.set_installs(ModelRc::new(VecModel::from(items)));
        api.set_detecting(false);
    });
}

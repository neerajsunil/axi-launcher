#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod launcher;

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;

use launcher::VersionEntry;
use slint::{ModelRc, SharedString, VecModel};

slint::include_modules!();

/// All versions from Mojang, plus the subset currently shown in the combo box.
#[derive(Default)]
struct Versions {
    all: Vec<VersionEntry>,
    visible: Vec<VersionEntry>,
}

fn refresh_versions(ui: &MainWindow, versions: &Mutex<Versions>) {
    let mut v = versions.lock().unwrap();
    let show_all = ui.get_show_snapshots();
    v.visible = v
        .all
        .iter()
        .filter(|e| show_all || e.kind == "release")
        .cloned()
        .collect();
    let names: Vec<SharedString> = v.visible.iter().map(|e| e.id.as_str().into()).collect();
    ui.set_versions(ModelRc::from(Rc::new(VecModel::from(names))));
    ui.set_selected_index(0);
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let versions = Arc::new(Mutex::new(Versions::default()));
    let client = launcher::http_client().expect("failed to create HTTP client");

    // Load the version list in the background.
    {
        let weak = ui.as_weak();
        let versions = versions.clone();
        let client = client.clone();
        thread::spawn(move || {
            let result = launcher::fetch_versions(&client);
            weak.upgrade_in_event_loop(move |ui| match result {
                Ok(list) => {
                    versions.lock().unwrap().all = list;
                    refresh_versions(&ui, &versions);
                    ui.set_status("Ready".into());
                }
                Err(e) => ui.set_status(format!("Failed to load versions: {e:#}").into()),
            })
            .ok();
        });
    }

    {
        let weak = ui.as_weak();
        let versions = versions.clone();
        ui.on_snapshots_toggled(move |_| {
            if let Some(ui) = weak.upgrade() {
                refresh_versions(&ui, &versions);
            }
        });
    }

    {
        let weak = ui.as_weak();
        ui.on_play(move |index, username| {
            let Some(ui) = weak.upgrade() else { return };
            let Some(version) = versions.lock().unwrap().visible.get(index as usize).cloned() else {
                return;
            };
            let username = match username.trim() {
                "" => "Player".to_string(),
                name => name.to_string(),
            };
            ui.set_busy(true);
            ui.set_progress(0.0);

            let weak = weak.clone();
            let client = client.clone();
            thread::spawn(move || {
                let report = |status: String, progress: f32| {
                    weak.upgrade_in_event_loop(move |ui| {
                        ui.set_status(status.into());
                        ui.set_progress(progress);
                    })
                    .ok();
                };
                let finish = |status: String| {
                    weak.upgrade_in_event_loop(move |ui| {
                        ui.set_status(status.into());
                        ui.set_busy(false);
                    })
                    .ok();
                };

                match launcher::launch(&client, &version, &username, &report) {
                    Ok(mut child) => {
                        finish(format!("Minecraft {} is running", version.id));
                        let msg = match child.wait() {
                            Ok(status) if status.success() => format!("Minecraft {} closed", version.id),
                            Ok(status) => format!(
                                "Minecraft {} exited with {status}. See {}",
                                version.id,
                                launcher::data_dir().join("logs").join("latest-launch.log").display()
                            ),
                            Err(e) => format!("Lost track of the game process: {e}"),
                        };
                        finish(msg);
                    }
                    Err(e) => finish(format!("Launch failed: {e:#}")),
                }
            });
        });
    }

    ui.run()
}

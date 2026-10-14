//! Play and Stop.

use std::sync::Arc;
use std::thread;

use axi_core::launch::{LaunchContext, OutputSink, launch};
use axi_core::util::now_unix;
use slint::ComponentHandle;

use crate::lan::Reason;
use crate::state::AppState;
use crate::{InstancesApi, MainWindow};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<InstancesApi>();
    {
        let (weak, state) = (ui.as_weak(), state.clone());
        api.on_play(move |id| play(weak.clone(), state.clone(), id.to_string()));
    }
    {
        let state = state.clone();
        api.on_stop(move |id| {
            let game = state.sessions().get_mut(id.as_str()).and_then(|s| {
                s.stop_requested = true;
                s.game.clone()
            });
            if let Some(game) = game {
                if let Err(e) = game.kill() {
                    { eprintln!("could not stop the game: {e}"); state.error("Couldn't stop the game") };
                }
            }
        });
    }
}

fn play(weak: slint::Weak<MainWindow>, state: Arc<AppState>, id: String) {
    if state.is_active(&id) {
        return;
    }
    if state.instance(&id).is_none() {
        return;
    }
    state.set_task(&id, Some(("Preparing...".into(), 0.0)));

    thread::spawn(move || {
        // Bring in what was played on other computers first; it can change the game version.
        crate::lan::sync(&state, &id, Reason::Launch);
        let Some(instance) = state.instance(&id) else {
            return state.set_task(&id, None);
        };
        let settings = state.settings().clone();
        let versions = state.versions.lock().unwrap().entries.clone();
        let ctx = LaunchContext { paths: &state.paths, client: &state.client, versions: &versions };
        let name = &instance.config.name;
        let account = match super::accounts::launch_account(&state) {
            Ok(account) => account,
            Err(e) => {
                state.set_task(&id, None);
                return state.error(format!("Couldn't start \"{name}\": {e}"));
            }
        };
        let progress = |status: String, p: f32| state.set_task(&id, Some((status, p)));
        let output: OutputSink = {
            let (state, id) = (state.clone(), id.clone());
            Arc::new(move |line| state.push_log(&id, line))
        };

        let game = match launch(&ctx, &instance, &settings, &account, &progress, output) {
            Ok(game) => game,
            Err(e) => {
                state.set_task(&id, None);
                state.push_log(&id, format!("[launcher] Launch failed: {e}"));
                return state.error(format!("Couldn't start \"{name}\": {e}"));
            }
        };
        {
            let mut sessions = state.sessions();
            let session = sessions.entry(id.clone()).or_default();
            session.task = None;
            session.game = Some(game.clone());
            session.stop_requested = false;
        }
        state.mark_dirty();
        state.lan.announce();
        let hide = settings.hide_on_launch;
        if hide {
            let _ = weak.upgrade_in_event_loop(|ui| {
                let _ = ui.hide();
            });
        }

        let status = game.wait();
        let stopped = {
            let mut sessions = state.sessions();
            let session = sessions.entry(id.clone()).or_default();
            session.game = None;
            std::mem::take(&mut session.stop_requested)
        };
        if let Ok(mut saved) = state.manager.get(&id) {
            saved.record_session(game.started, now_unix());
            let _ = saved.save();
        }
        state.reload_instances();

        match status {
            Ok(code) if code.success() || stopped => {
                state.push_log(&id, "[launcher] Game closed".into());
            }
            Ok(code) => {
                state.push_log(&id, format!("[launcher] Game exited with {code}"));
                state.error(format!("\"{name}\" crashed. Check Logs for clues."));
            }
            Err(e) => { eprintln!("lost track of the game: {e}"); state.error("Lost track of the game") },
        }
        if hide {
            let _ = weak.upgrade_in_event_loop(|ui| {
                let _ = ui.show();
            });
        }
        crate::lan::after_exit(&state, &id);
    });
}

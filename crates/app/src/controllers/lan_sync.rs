//! LAN sync buttons: turning it on per instance, syncing now, adding instances from other computers.

use std::sync::Arc;
use std::thread;

use slint::ComponentHandle;

use crate::lan::{self, Reason};
use crate::state::AppState;
use crate::{InstancesApi, MainWindow};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<InstancesApi>();
    {
        let state = state.clone();
        api.on_set_lan_sync(move |id, on| lan::set_enabled(&state, &id, on));
    }
    {
        let state = state.clone();
        api.on_sync_now(move |id| {
            if state.is_active(&id) {
                return state.error("Wait until the game closes to sync");
            }
            let (state, id) = (state.clone(), id.to_string());
            thread::spawn(move || lan::sync(&state, &id, Reason::Manual));
        });
    }
    {
        let state = state.clone();
        api.on_add_lan_instance(move |sync_id| lan::add(&state, &sync_id));
    }
}

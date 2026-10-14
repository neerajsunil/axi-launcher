//! The Settings tab of an instance.

use std::sync::Arc;

use slint::ComponentHandle;

use super::instances::select;
use crate::convert;
use crate::state::AppState;
use crate::{InstancesApi, MainWindow};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let (weak, state) = (ui.as_weak(), state.clone());
    ui.global::<InstancesApi>().on_save_settings(move |id, data| {
        let Some(ui) = weak.upgrade() else { return };
        let mut instance = match state.manager.get(&id) {
            Ok(instance) => instance,
            Err(e) => return state.error(e.to_string()),
        };
        if let Err(e) = convert::apply_instance_settings(&data, &mut instance.config) {
            return state.error(e);
        }
        if let Err(e) = instance.save() {
            return { eprintln!("could not save instance: {e}"); state.error("Couldn't save") };
        }
        state.reload_instances();
        select(&ui, &state, &id);
        state.info("Saved");
    });
}

//! Instance pictures: one picked by the user, or the logo of the modpack it came from.
//! Both end up as `icon.png` in the instance folder.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use axi_core::instance::Instance;
use axi_core::mods::ModApi;
use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::state::AppState;

/// Pictures are stored at most this many pixels wide.
const SIZE: u32 = 128;

/// Decodes an instance's `icon.png`.
pub fn load(path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let image = image::open(path).ok()?.thumbnail(SIZE, SIZE).to_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(image.as_raw(), image.width(), image.height()))
}

/// Saves the picture at `source` (any common format) as the instance's icon.
pub fn set(instance: &Instance, source: &Path) -> anyhow::Result<()> {
    let image = image::open(source)?;
    image.thumbnail(SIZE, SIZE).save(instance.icon_file())?;
    Ok(())
}

/// Downloads the modpack logo of an instance installed from a site, once per run, if it has no icon yet.
pub fn fetch_pack_icon(state: &Arc<AppState>, instance: &Instance) {
    let Some(pack) = &instance.config.pack else { return };
    let Some(source) = pack.source else { return };
    if instance.icon_file().exists() || !state.pack_icons.lock().unwrap().insert(instance.id.clone()) {
        return;
    }
    let (state, instance, project_id) = (state.clone(), instance.clone(), pack.project_id.clone());
    rayon::spawn(move || {
        let result = (|| -> anyhow::Result<()> {
            let key = state.settings().curseforge_key();
            let api = ModApi { client: &state.client, curseforge_key: &key };
            let Some(url) = api.project(source, &project_id)?.icon_url else { return Ok(()) };
            let bytes = state.client.get(url).send()?.error_for_status()?.bytes()?;
            let image = image::load_from_memory(&bytes)?.thumbnail(SIZE, SIZE);
            // The user may have picked a picture meanwhile.
            if !instance.icon_file().exists() {
                image.save(instance.icon_file())?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => state.mark_dirty(),
            Err(e) => eprintln!("could not get the modpack logo of {}: {e}", instance.id),
        }
    });
}

/// Removes a picked picture; modpack instances get their pack logo back.
pub fn reset(state: &AppState, instance: &Instance) -> std::io::Result<()> {
    match fs::remove_file(instance.icon_file()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    state.pack_icons.lock().unwrap().remove(&instance.id);
    Ok(())
}

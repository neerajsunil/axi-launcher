//! Mod logos and description images: downloaded once into the cache folder and decoded off the UI thread.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use slint::{Rgba8Pixel, SharedPixelBuffer};

use crate::state::AppState;

/// Logos are shown at most this many pixels wide.
pub const LOGO: u32 = 96;
/// Description and gallery pictures are kept at most this big.
pub const PICTURE: u32 = 900;

enum Icon {
    Loading,
    Ready(SharedPixelBuffer<Rgba8Pixel>),
    Failed,
}

pub struct IconStore {
    dir: PathBuf,
    entries: Mutex<HashMap<String, Icon>>,
}

impl IconStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, entries: Mutex::default() }
    }
}

/// The image at `url` scaled to fit `size`, or `None` while it loads (the first call starts loading it).
pub fn get(state: &Arc<AppState>, url: &str, size: u32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let key = format!("{size}:{url}");
    let mut entries = state.icons.entries.lock().unwrap();
    match entries.get(&key) {
        Some(Icon::Ready(buffer)) => Some(buffer.clone()),
        Some(Icon::Loading | Icon::Failed) => None,
        None => {
            entries.insert(key.clone(), Icon::Loading);
            let (state, url) = (state.clone(), url.to_string());
            rayon::spawn(move || {
                let icon = load(&state, &url, size).map_or(Icon::Failed, Icon::Ready);
                state.icons.entries.lock().unwrap().insert(key, icon);
                state.mark_dirty();
            });
            None
        }
    }
}

/// Drops decoded images whose `size:url` key isn't in `keep` (they stay cached on disk).
pub fn retain(state: &AppState, keep: &HashSet<String>) {
    state.icons.entries.lock().unwrap().retain(|key, _| keep.contains(key));
}

fn load(state: &AppState, url: &str, size: u32) -> anyhow::Result<SharedPixelBuffer<Rgba8Pixel>> {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let path = state.icons.dir.join(format!("{:016x}", hasher.finish()));
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let bytes = state.client.get(url).send()?.error_for_status()?.bytes()?.to_vec();
            fs::create_dir_all(&state.icons.dir)?;
            fs::write(&path, &bytes)?;
            bytes
        }
    };
    let image = image::load_from_memory(&bytes)?;
    // Only ever scaled down, so small images keep their size on screen.
    let image = if image.width() > size || image.height() > size { image.thumbnail(size, size) } else { image };
    let image = image.to_rgba8();
    Ok(SharedPixelBuffer::clone_from_slice(image.as_raw(), image.width(), image.height()))
}

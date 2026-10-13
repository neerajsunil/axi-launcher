//! Player heads: the face of each Microsoft account's skin, cut out off the UI thread.
//! The last head seen is kept in the cache folder so it shows at once on the next start.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use image::{GenericImageView, RgbaImage, imageops};
use slint::{Rgba8Pixel, SharedPixelBuffer};

use axi_core::account::skin;

use crate::state::AppState;

/// A head and how many times it has changed, so the UI knows when to rebuild its image.
pub type Head = (u32, SharedPixelBuffer<Rgba8Pixel>);

pub struct HeadStore {
    dir: PathBuf,
    entries: Mutex<HashMap<String, Option<Head>>>,
}

impl HeadStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, entries: Mutex::default() }
    }
}

/// The head of the player with `uuid`, or `None` until it loads (the first call starts loading it).
pub fn get(state: &Arc<AppState>, uuid: &str) -> Option<Head> {
    let mut entries = state.heads.entries.lock().unwrap();
    if let Some(head) = entries.get(uuid) {
        return head.clone();
    }
    entries.insert(uuid.to_string(), None);
    let (state, uuid) = (state.clone(), uuid.to_string());
    rayon::spawn(move || {
        let path = state.heads.dir.join(format!("{uuid}.png"));
        if let Some(face) = fs::read(&path).ok().and_then(|bytes| image::load_from_memory(&bytes).ok()) {
            set(&state, &uuid, face.to_rgba8());
        }
        match download(&state, &uuid) {
            Ok(Some(face)) => {
                let _ = fs::create_dir_all(&state.heads.dir).and_then(|_| face.save(&path).map_err(std::io::Error::other));
                set(&state, &uuid, face);
            }
            Ok(None) => {}
            Err(e) => eprintln!("could not load the skin of {uuid}: {e}"),
        }
    });
    None
}

fn set(state: &AppState, uuid: &str, face: RgbaImage) {
    let buffer = SharedPixelBuffer::clone_from_slice(face.as_raw(), face.width(), face.height());
    let mut entries = state.heads.entries.lock().unwrap();
    let version = entries.get(uuid).and_then(|h| h.as_ref()).map_or(0, |h| h.0 + 1);
    entries.insert(uuid.to_string(), Some((version, buffer)));
    state.mark_dirty();
}

/// Fetches the skin and cuts out the face; `None` when the player has the default skin.
fn download(state: &AppState, uuid: &str) -> anyhow::Result<Option<RgbaImage>> {
    let Some(url) = skin::skin_url(&state.client, uuid)? else { return Ok(None) };
    let bytes = state.client.get(url).send()?.error_for_status()?.bytes()?;
    Ok(Some(face(&image::load_from_memory(&bytes)?.to_rgba8())))
}

/// The 8×8 face with the hat layer drawn over it.
fn face(skin: &RgbaImage) -> RgbaImage {
    let mut face = skin.view(8, 8, 8, 8).to_image();
    imageops::overlay(&mut face, &*skin.view(40, 8, 8, 8), 0, 0);
    face
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn hat_layer_covers_face() {
        let mut skin = RgbaImage::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
        for (x, y) in [(8, 8), (9, 8)] {
            skin.put_pixel(x, y, Rgba([255, 0, 0, 255]));
        }
        skin.put_pixel(41, 8, Rgba([0, 0, 255, 255]));
        let face = face(&skin);
        assert_eq!(face.dimensions(), (8, 8));
        assert_eq!(face.get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
        assert_eq!(face.get_pixel(1, 0), &Rgba([0, 0, 255, 255]));
    }
}

use std::fs::{self, File};
use std::io;
use std::path::Path;

use crate::Result;

/// Extracts native libraries from `jar` into `dest`. Existing files are left alone
/// because a running game of the same version keeps them locked on Windows.
pub fn extract_natives(jar: &Path, dest: &Path, exclude: &[String]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(File::open(jar)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name()?.into_owned();
        if entry.is_dir() || exclude.iter().any(|ex| name.starts_with(ex.as_str())) {
            continue;
        }
        let Some(rel) = entry.enclosed_name() else { continue };
        let out_path = dest.join(rel);
        if out_path.exists() {
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut File::create(&out_path)?)?;
    }
    Ok(())
}

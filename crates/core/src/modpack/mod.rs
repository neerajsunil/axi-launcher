//! Modpacks: Modrinth `.mrpack` and CurseForge `.zip` files that describe a whole instance.
//!
//! Installing one is split in steps so the app can create the instance (and show its
//! progress) before the slow part: [`download`] the pack, [`open`] it to learn the game
//! it needs, then [`install`] its files into a fresh instance.

mod curseforge;
mod mrpack;

use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use zip::ZipArchive;

use crate::instance::{GameSetup, Instance, PackOrigin};
use crate::loader::ModLoader;
use crate::mods::{ContentKind, ModApi, ModSource};
use crate::net::download as download_file;
use crate::{Error, Progress, Result};

/// A pack file that couldn't be downloaded automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    pub title: String,
    pub page_url: String,
}

/// An opened pack, ready to install.
#[derive(Debug)]
pub struct Modpack {
    pub name: String,
    pub version: String,
    pub game: GameSetup,
    archive: PathBuf,
    format: Format,
}

#[derive(Debug)]
enum Format {
    Modrinth(mrpack::Index),
    CurseForge(curseforge::Manifest),
}

/// Downloads the newest version of a pack from Modrinth or CurseForge into `dir`.
pub fn download(
    api: &ModApi,
    source: ModSource,
    project_id: &str,
    dir: &Path,
    progress: Progress,
) -> Result<(PathBuf, PackOrigin)> {
    progress("Finding the newest version...".into(), 0.0);
    let info = api.project(source, project_id)?;
    let file = api
        .latest_file(source, ContentKind::Modpack, project_id, "", ModLoader::Vanilla)?
        .ok_or_else(|| Error::msg(format!("{} has no version for Fabric or NeoForge", info.title)))?;
    let url = file.url.ok_or_else(|| {
        Error::msg(format!("The author of {} only allows downloads from the CurseForge website", info.title))
    })?;
    progress(format!("Downloading {}...", info.title), 0.05);
    let path = dir.join(format!("{}-{}", source.display_name(), file.file_id));
    download_file(api.client, &url, &path, file.size)?;
    let origin = PackOrigin {
        name: info.title,
        version: file.version_number,
        source: Some(source),
        project_id: file.project_id,
        file_id: file.file_id,
    };
    Ok((path, origin))
}

/// Reads a pack file and checks that this launcher can run it.
pub fn open(path: &Path) -> Result<Modpack> {
    let mut zip = ZipArchive::new(File::open(path)?)?;
    let (name, version, game, format) = if let Ok(entry) = zip.by_name(mrpack::INDEX) {
        let index: mrpack::Index = serde_json::from_reader(entry)?;
        let game = mrpack::game(&index)?;
        (index.name.clone(), index.version_id.clone(), game, Format::Modrinth(index))
    } else if let Ok(entry) = zip.by_name(curseforge::MANIFEST) {
        let manifest: curseforge::Manifest = serde_json::from_reader(entry)?;
        let game = curseforge::game(&manifest)?;
        (manifest.name.clone(), manifest.version.clone(), game, Format::CurseForge(manifest))
    } else {
        return Err(Error::msg("This isn't a Modrinth or CurseForge modpack"));
    };
    if game.loader == ModLoader::NeoForge && game.version == "1.20.1" {
        return Err(Error::msg("NeoForge 1.20.1 packs aren't supported."));
    }
    let name = if name.trim().is_empty() { "Modpack".to_string() } else { name.trim().to_string() };
    Ok(Modpack { name, version, game, archive: path.to_path_buf(), format })
}

/// Downloads the pack's files into `instance` (made for `pack.game`) and copies its
/// config overrides. Returns files the user has to download by hand.
pub fn install(
    api: &ModApi,
    pack: &Modpack,
    instance: &Instance,
    threads: usize,
    progress: Progress,
) -> Result<Vec<SkippedFile>> {
    let game_dir = instance.game_dir();
    let mut zip = ZipArchive::new(File::open(&pack.archive)?)?;
    let skipped = match &pack.format {
        Format::Modrinth(index) => {
            mrpack::install(api.client, index, instance, threads, progress)?;
            extract_folder(&mut zip, "overrides", &game_dir)?;
            extract_folder(&mut zip, "client-overrides", &game_dir)?;
            Vec::new()
        }
        Format::CurseForge(manifest) => {
            let skipped = curseforge::install(api, manifest, instance, threads, progress)?;
            extract_folder(&mut zip, &manifest.overrides, &game_dir)?;
            skipped
        }
    };
    progress("Done".into(), 1.0);
    Ok(skipped)
}

/// `base/relative`, refusing paths that would land outside `base`.
fn safe_join(base: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    let ok = !relative.is_empty()
        && !relative.contains([':', '\\'])
        && path.components().all(|c| matches!(c, Component::Normal(_)));
    if !ok {
        return Err(Error::msg(format!("the pack has an unsafe file path '{relative}'")));
    }
    Ok(base.join(path))
}

/// Copies everything under `folder/` in the archive into `dest`.
fn extract_folder(zip: &mut ZipArchive<File>, folder: &str, dest: &Path) -> Result<()> {
    let prefix = format!("{}/", folder.trim_end_matches('/'));
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(name) = entry.enclosed_name() else { continue };
        let Ok(relative) = name.strip_prefix(&prefix) else { continue };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            io::copy(&mut entry, &mut File::create(&target)?)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_zip(path: &Path, files: &[(&str, &str)]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, text) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn opens_both_formats() {
        let tmp = tempfile::tempdir().unwrap();
        let mrpack = tmp.path().join("a.mrpack");
        write_zip(
            &mrpack,
            &[(
                "modrinth.index.json",
                r#"{"formatVersion":1,"game":"minecraft","versionId":"1.2","name":"Fabulous",
                    "files":[],"dependencies":{"minecraft":"1.21.1","fabric-loader":"0.16.5"}}"#,
            )],
        );
        let pack = open(&mrpack).unwrap();
        assert_eq!((pack.name.as_str(), pack.version.as_str()), ("Fabulous", "1.2"));
        assert_eq!(pack.game.loader, ModLoader::Fabric);
        assert_eq!(pack.game.loader_version, "0.16.5");

        let cf = tmp.path().join("b.zip");
        write_zip(
            &cf,
            &[(
                "manifest.json",
                r#"{"minecraft":{"version":"1.21.1","modLoaders":[{"id":"neoforge-21.1.77","primary":true}]},
                    "manifestType":"minecraftModpack","name":"Neo","version":"3","files":[],"overrides":"overrides"}"#,
            )],
        );
        let pack = open(&cf).unwrap();
        assert_eq!(pack.game.loader, ModLoader::NeoForge);
        assert_eq!(pack.game.loader_version, "21.1.77");

        let forge = tmp.path().join("c.zip");
        write_zip(
            &forge,
            &[(
                "manifest.json",
                r#"{"minecraft":{"version":"1.20.1","modLoaders":[{"id":"forge-47.2.0","primary":true}]},
                    "name":"Old","version":"1","files":[]}"#,
            )],
        );
        assert!(open(&forge).unwrap_err().to_string().contains("Forge"));

        let other = tmp.path().join("d.zip");
        write_zip(&other, &[("readme.txt", "hi")]);
        assert!(open(&other).is_err());
    }

    #[test]
    fn extracts_overrides_safely() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = tmp.path().join("p.zip");
        write_zip(&archive, &[("overrides/config/a.toml", "a=1"), ("overrides/options.txt", "fov:90"), ("other/x", "x")]);
        let dest = tmp.path().join("game");
        extract_folder(&mut ZipArchive::new(File::open(&archive).unwrap()).unwrap(), "overrides", &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("config/a.toml")).unwrap(), "a=1");
        assert!(dest.join("options.txt").exists());
        assert!(!dest.join("x").exists());

        assert!(safe_join(&dest, "mods/a.jar").is_ok());
        assert!(safe_join(&dest, "../evil.jar").is_err());
        assert!(safe_join(&dest, "/etc/passwd").is_err());
        assert!(safe_join(&dest, "C:/x.jar").is_err());
    }
}

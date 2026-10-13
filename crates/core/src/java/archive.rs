//! Downloading and unpacking JRE archives (Temurin and Zulu builds).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use super::detect::find_java;
use crate::{Error, Progress, Result};

/// A `.zip` or `.tar.gz` holding one JRE under a single top-level folder.
pub(crate) struct Archive<'a> {
    pub name: &'a str,
    pub url: &'a str,
    pub sha256: &'a str,
}

/// The java executable in `dest` if a previous install finished.
pub(crate) fn installed(dest: &Path) -> Option<PathBuf> {
    dest.join(".complete").exists().then(|| find_java(dest)).flatten()
}

/// Downloads `archive`, checks it and unpacks it into `dest`; returns its java executable.
pub(crate) fn install(
    client: &Client,
    runtimes_dir: &Path,
    archive: &Archive,
    dest: &Path,
    label: &str,
    progress: Progress,
) -> Result<PathBuf> {
    fs::create_dir_all(runtimes_dir)?;
    // The name comes from a web API; keep only its last component.
    let file_name = Path::new(archive.name).file_name().unwrap_or("java-download".as_ref());
    let file = runtimes_dir.join(file_name);
    let result = download_verified(client, archive, &file, label, progress).and_then(|()| {
        progress(format!("Unpacking {label}..."), 1.0);
        unpack(&file, dest)
    });
    let _ = fs::remove_file(&file);
    result?;

    let java = find_java(dest).ok_or_else(|| Error::msg(format!("{label} archive has no java executable")))?;
    File::create(dest.join(".complete"))?;
    Ok(java)
}

/// Streams `archive` to `path`, reporting progress and checking its SHA-256.
fn download_verified(client: &Client, archive: &Archive, path: &Path, label: &str, progress: Progress) -> Result<()> {
    let mut resp = client.get(archive.url).send()?.error_for_status()?;
    let size = resp.content_length().unwrap_or(0);
    let mut file = File::create(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    let mut done = 0u64;
    let mut last_mb = u64::MAX;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if done >> 20 != last_mb {
            last_mb = done >> 20;
            let (mb, total_mb) = (done >> 20, size >> 20);
            progress(format!("Downloading {label} ({mb}/{total_mb} MB)"), done as f32 / size.max(1) as f32);
        }
    }
    let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !hash.eq_ignore_ascii_case(archive.sha256) {
        return Err(Error::msg(format!("{} is corrupted (checksum mismatch)", archive.name)));
    }
    Ok(())
}

/// Drops the archive's top-level folder (`jdk-21.0.4+7-jre/...`), refusing paths that escape `dest`.
fn strip_top(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    parts.next()?;
    let rest: PathBuf = parts.collect();
    let safe = rest.components().all(|c| matches!(c, Component::Normal(_)));
    (safe && !rest.as_os_str().is_empty()).then_some(rest)
}

/// Unpacks a `.zip` or `.tar.gz` into `dest`, replacing whatever was there.
fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    fs::create_dir_all(&tmp)?;

    if archive.extension().is_some_and(|e| e == "zip") {
        let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(rel) = entry.enclosed_name().as_deref().and_then(strip_top) else { continue };
            let out = tmp.join(rel);
            if entry.is_dir() {
                fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            io::copy(&mut entry, &mut File::create(&out)?)?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&out, fs::Permissions::from_mode(mode))?;
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(File::open(archive)?));
        for entry in tar.entries()? {
            let mut entry = entry?;
            let Some(rel) = strip_top(&entry.path()?) else { continue };
            let out = tmp.join(rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(&out)?;
        }
    }

    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::rename(&tmp, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_top_folder() {
        assert_eq!(strip_top(Path::new("jdk-21+7-jre/bin/java")), Some(PathBuf::from("bin/java")));
        assert_eq!(strip_top(Path::new("jdk-21+7-jre")), None);
        assert_eq!(strip_top(Path::new("jdk/../../evil")), None);
    }

    #[test]
    fn unpacks_zip_without_top_folder() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("jre.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive).unwrap());
        zip.start_file("jdk-21-jre/release", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"JAVA_VERSION=\"21\"").unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("temurin-21");
        fs::create_dir_all(dest.join("stale")).unwrap();
        unpack(&archive, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("release")).unwrap(), "JAVA_VERSION=\"21\"");
        assert!(!dest.join("stale").exists());
    }
}

//! Brings a peer's changes to an instance here: merge, fetch into a staging folder, then
//! swap the files in. Everything replaced or deleted goes into a backup folder first.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use super::Device;
use super::connection::Connection;
use super::manifest::{self, FileEntry, META_PATH, Manifest, sync_dir};
use super::merge::{self, Devices};
use super::rules;
use crate::instance::Instance;
use crate::mods::ModApi;
use crate::net;
use crate::util::{now_unix, write_atomic};
use crate::{Error, Progress, Result};

/// Backups of replaced files kept per instance.
const BACKUPS_KEPT: usize = 3;

#[derive(Debug, Default)]
pub struct PullReport {
    /// Files added, replaced, moved or deleted here.
    pub changed: usize,
    /// What both computers had changed (world names or file paths).
    pub conflicts: Vec<String>,
    /// The game version, loader or name changed.
    pub meta_changed: bool,
}

/// Pulls `sync_id` from the peer on `conn` into `instance`. `api` lets mods and packs be
/// downloaded from Modrinth/CurseForge instead of copied from the peer.
pub fn pull(
    conn: &mut Connection,
    me: &Device,
    instance: &mut Instance,
    sync_id: &str,
    api: Option<&ModApi>,
    progress: Progress,
) -> Result<PullReport> {
    let peer = conn.peer.clone();
    progress(format!("Comparing with {}...", peer.name), 0.0);
    let remote = conn.manifest(sync_id)?;
    for file in &remote.files {
        rules::check(&file.path)?;
    }
    let local = manifest::scan(instance)?;
    let base_file = sync_dir(instance).join(format!("base-{}.json", safe_name(&peer.id)));
    let base: BTreeMap<String, String> = match fs::read(&base_file).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(base) => base,
        // First pull from this peer: its game setup wins, since this copy is the newcomer.
        None => BTreeMap::from([(META_PATH.to_string(), local.meta.hash())]),
    };

    let mut plan = merge::plan(&base, &local.versions(), &remote.versions(), Devices { local: &me.name, remote: &peer.name });
    let take_meta = plan.take.iter().any(|(from, _)| from == META_PATH);
    plan.take.retain(|(from, _)| from != META_PATH);
    plan.delete.retain(|path| path != META_PATH);
    let mut report = PullReport { conflicts: plan.conflicts.clone(), meta_changed: take_meta, ..Default::default() };

    if !plan.is_empty() {
        let staging = sync_dir(instance).join("staging");
        let _ = fs::remove_dir_all(&staging);
        fetch_all(conn, instance, sync_id, &remote, &local, &plan.take, &staging, api, &peer, progress)?;

        progress("Applying changes...".into(), 0.95);
        let backup = sync_dir(instance).join("backup").join(now_unix().to_string());
        for (from, to) in &plan.rename {
            let from = rules::on_disk(&instance.dir, from);
            if from.exists() {
                fs::rename(&from, unique_dir(&rules::on_disk(&instance.dir, to)))?;
            }
        }
        for path in &plan.delete {
            move_into(&rules::on_disk(&instance.dir, path), &rules::on_disk(&backup, path))?;
        }
        for (_, to) in &plan.take {
            let dest = rules::on_disk(&instance.dir, to);
            move_into(&dest, &rules::on_disk(&backup, to))?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(rules::on_disk(&staging, to), &dest)?;
        }
        let _ = fs::remove_dir_all(&staging);
        prune_backups(&sync_dir(instance).join("backup"));
        report.changed = plan.rename.len() + plan.delete.len() + plan.take.len();
    }

    if take_meta {
        instance.reload()?;
        remote.meta.apply(&mut instance.config);
        instance.save()?;
    }
    write_atomic(&base_file, serde_json::to_vec(&remote.hashes())?)?;
    progress("Synced".into(), 1.0);
    Ok(report)
}

/// Fetches every file in `take` into `staging`: from an identical file here, from
/// Modrinth/CurseForge, or from the peer, in that order. Each is checked against its hash.
#[allow(clippy::too_many_arguments)]
fn fetch_all(
    conn: &mut Connection,
    instance: &Instance,
    sync_id: &str,
    remote: &Manifest,
    local: &Manifest,
    take: &[(String, String)],
    staging: &Path,
    api: Option<&ModApi>,
    peer: &Device,
    progress: Progress,
) -> Result<()> {
    let by_hash: HashMap<&str, &str> = local.files.iter().map(|f| (f.hash.as_str(), f.path.as_str())).collect();
    let total = take.len();
    let done = AtomicUsize::new(0);
    let tick = |label: &str| {
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        progress(format!("{label} ({n}/{total})..."), 0.9 * n as f32 / total.max(1) as f32);
    };

    let mut missing = Vec::new();
    let mut online = Vec::new();
    for (from, to) in take {
        let entry = remote.file(from).ok_or_else(|| Error::msg(format!("{from} is not in the manifest")))?;
        let staged = rules::on_disk(staging, to);
        if let Some(parent) = staged.parent() {
            fs::create_dir_all(parent)?;
        }
        let copied = by_hash
            .get(entry.hash.as_str())
            .is_some_and(|path| fs::copy(rules::on_disk(&instance.dir, path), &staged).is_ok());
        if copied {
            finish(&staged, entry)?;
            tick("Copying");
        } else if entry.origin.is_some() && api.is_some() {
            online.push((entry, staged));
        } else {
            missing.push((entry, staged));
        }
    }

    if let Some(api) = api {
        let failed: Vec<_> = net::pool(8)?.install(|| {
            online
                .into_par_iter()
                .filter(|(entry, staged)| {
                    let ok = download(api, entry, staged).is_ok() && finish(staged, entry).is_ok();
                    if ok {
                        tick("Downloading");
                    }
                    !ok
                })
                .collect()
        });
        missing.extend(failed);
    }

    for (entry, staged) in missing {
        let mut out = BufWriter::new(File::create(&staged)?);
        conn.fetch(sync_id, &entry.path, &mut out)?;
        drop(out);
        finish(&staged, entry)?;
        tick(&format!("Copying from {}", peer.name));
    }
    Ok(())
}

fn download(api: &ModApi, entry: &FileEntry, staged: &Path) -> Result<()> {
    let origin = entry.origin.as_ref().ok_or_else(|| Error::msg("no origin"))?;
    let file = api
        .file(origin.source, origin.kind, &origin.project_id, &origin.file_id)?
        .ok_or_else(|| Error::msg("file is gone"))?;
    let url = file.url.ok_or_else(|| Error::msg("no download link"))?;
    let _ = fs::remove_file(staged);
    net::download(api.client, &url, staged, None)
}

/// Checks a fetched file against its hash and gives it the peer's modification time,
/// so the next merge sees the same version on both sides.
fn finish(staged: &Path, entry: &FileEntry) -> Result<()> {
    if manifest::hash_file(staged)? != entry.hash {
        let _ = fs::remove_file(staged);
        return Err(Error::msg(format!("{} arrived damaged", entry.path)));
    }
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_millis(entry.modified);
    File::options().write(true).open(staged)?.set_modified(time)?;
    Ok(())
}

/// Moves `from` (if it exists) to `to`, making folders as needed.
fn move_into(from: &Path, to: &Path) -> Result<()> {
    if !from.exists() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(from, to)?;
    Ok(())
}

/// `dir`, or `dir 2`, `dir 3`, ... whichever doesn't exist.
fn unique_dir(dir: &Path) -> PathBuf {
    if !dir.exists() {
        return dir.to_path_buf();
    }
    let mut name = dir.as_os_str().to_owned();
    name.push(" ");
    (2..).map(|n| {
        let mut candidate = name.clone();
        candidate.push(n.to_string());
        PathBuf::from(candidate)
    })
    .find(|p| !p.exists())
    .unwrap()
}

fn prune_backups(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut backups: Vec<(u64, PathBuf)> = entries
        .flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, e.path())))
        .collect();
    backups.sort();
    let excess = backups.len().saturating_sub(BACKUPS_KEPT);
    for (_, path) in backups.into_iter().take(excess) {
        let _ = fs::remove_dir_all(path);
    }
}

/// A device id as a file name.
fn safe_name(id: &str) -> String {
    id.chars().filter(char::is_ascii_alphanumeric).take(64).collect()
}

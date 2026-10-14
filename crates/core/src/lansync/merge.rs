//! Decides what pulling from a peer changes here: a three-way merge of this computer's
//! files, the peer's, and the peer's files as of the last pull (the base).
//!
//! A world is one unit: if both computers played it since the last sync, mixing their
//! region files would corrupt it, so the newer copy keeps the name and the other is kept
//! beside it as "World (Computer)". Both computers make the same choice, so they converge.

use std::collections::{BTreeMap, BTreeSet};

use super::manifest::Version;
use super::rules::world_of;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// World folders here to move aside before anything else: (from, to).
    pub rename: Vec<(String, String)>,
    /// Local files to delete (moved into the backup).
    pub delete: Vec<String>,
    /// Peer files to fetch: (peer path, local path).
    pub take: Vec<(String, String)>,
    /// What both sides changed, for telling the user.
    pub conflicts: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.rename.is_empty() && self.delete.is_empty() && self.take.is_empty()
    }
}

/// Names of the two computers, used for the copy a world conflict leaves behind.
pub struct Devices<'a> {
    pub local: &'a str,
    pub remote: &'a str,
}

pub fn plan(
    base: &BTreeMap<String, String>,
    local: &BTreeMap<String, Version>,
    remote: &BTreeMap<String, Version>,
    devices: Devices,
) -> Plan {
    let hash = |map: &BTreeMap<String, Version>, path: &str| map.get(path).map(|v| v.hash.clone());
    let paths: BTreeSet<&String> = base.keys().chain(local.keys()).chain(remote.keys()).collect();

    // Which side changed each world, counting only files that differ between the sides.
    let mut local_changed = BTreeSet::new();
    let mut remote_changed = BTreeSet::new();
    for path in &paths {
        let Some(world) = world_of(path) else { continue };
        let (l, r, b) = (hash(local, path), hash(remote, path), base.get(*path).cloned());
        if l != r {
            if l != b {
                local_changed.insert(world);
            }
            if r != b {
                remote_changed.insert(world);
            }
        }
    }
    let conflicted: BTreeSet<&str> = local_changed.intersection(&remote_changed).copied().collect();

    let mut plan = Plan::default();
    for path in &paths {
        let path = path.as_str();
        if world_of(path).is_some_and(|w| conflicted.contains(w)) {
            continue;
        }
        let (l, r, b) = (local.get(path), remote.get(path), base.get(path));
        let (lh, rh) = (l.map(|v| &v.hash), r.map(|v| &v.hash));
        if lh == rh {
            continue;
        }
        let remote_wins = if lh == b {
            true
        } else if rh == b {
            false
        } else {
            plan.conflicts.push(path.to_string());
            // Changed on both sides: a change beats a deletion, then the newer one wins.
            match (l, r) {
                (_, None) => false,
                (None, _) => true,
                (Some(l), Some(r)) => newer(r.modified, &r.hash, l.modified, &l.hash),
            }
        };
        if remote_wins {
            match r {
                Some(_) => plan.take.push((path.to_string(), path.to_string())),
                None => plan.delete.push(path.to_string()),
            }
        }
    }

    for world in conflicted {
        let stamp = |map: &BTreeMap<String, Version>| {
            let files = map.iter().filter(|(p, _)| world_of(p) == Some(world));
            let modified = files.clone().map(|(_, v)| v.modified).max().unwrap_or(0);
            let hash: String = files.map(|(_, v)| v.hash.as_str()).collect();
            (modified, hash)
        };
        let ((rm, rh), (lm, lh)) = (stamp(remote), stamp(local));
        let name = world.rsplit('/').next().unwrap_or(world);
        plan.conflicts.push(name.to_string());
        let in_world = |p: &&String| world_of(p) == Some(world);
        if newer(rm, &rh, lm, &lh) {
            // The peer's world takes the name; this one moves aside.
            let aside = format!("{world} ({})", devices.local);
            plan.rename.push((world.to_string(), aside));
            plan.take.extend(remote.keys().filter(in_world).map(|p| (p.clone(), p.clone())));
        } else {
            // This world keeps the name; the peer's comes along beside it.
            let aside = format!("{world} ({})", devices.remote);
            plan.take.extend(remote.keys().filter(in_world).map(|p| (p.clone(), format!("{aside}{}", &p[world.len()..]))));
        }
    }
    plan
}

/// Whether version `a` beats `b`: later modification, ties broken by hash so both
/// computers decide the same way.
fn newer(a_modified: u64, a_hash: &str, b_modified: u64, b_hash: &str) -> bool {
    (a_modified, a_hash) > (b_modified, b_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: Devices = Devices { local: "Desktop", remote: "Laptop" };

    fn files(list: &[(&str, &str, u64)]) -> BTreeMap<String, Version> {
        list.iter().map(|(p, h, m)| (p.to_string(), Version { hash: h.to_string(), modified: *m })).collect()
    }

    fn base(list: &[(&str, &str)]) -> BTreeMap<String, String> {
        list.iter().map(|(p, h)| (p.to_string(), h.to_string())).collect()
    }

    const LEVEL: &str = ".minecraft/saves/W/level.dat";
    const REGION: &str = ".minecraft/saves/W/region/r.0.0.mca";

    #[test]
    fn first_pull_takes_everything() {
        let remote = files(&[(LEVEL, "a", 1), (".minecraft/mods/x.jar", "j", 1)]);
        let plan = plan(&BTreeMap::new(), &BTreeMap::new(), &remote, DEV);
        assert_eq!(plan.take.len(), 2);
        assert!(plan.conflicts.is_empty() && plan.delete.is_empty());
    }

    #[test]
    fn one_sided_changes() {
        let b = base(&[(LEVEL, "a"), (".minecraft/mods/old.jar", "o"), (".minecraft/servers.dat", "s")]);
        // Peer changed the world and removed a mod; we changed the server list.
        let local = files(&[(LEVEL, "a", 1), (".minecraft/mods/old.jar", "o", 1), (".minecraft/servers.dat", "s2", 5)]);
        let remote = files(&[(LEVEL, "b", 2), (".minecraft/servers.dat", "s", 1)]);
        let plan = plan(&b, &local, &remote, DEV);
        assert_eq!(plan.take, [(LEVEL.to_string(), LEVEL.to_string())]);
        assert_eq!(plan.delete, [".minecraft/mods/old.jar"]);
        assert!(plan.conflicts.is_empty());
    }

    #[test]
    fn nothing_to_do_when_equal() {
        let local = files(&[(LEVEL, "a", 1)]);
        assert!(plan(&BTreeMap::new(), &local, &local.clone(), DEV).is_empty());
    }

    #[test]
    fn world_played_on_both_keeps_both() {
        let b = base(&[(LEVEL, "a"), (REGION, "r")]);
        let local = files(&[(LEVEL, "a", 1), (REGION, "r-local", 10)]);
        let remote = files(&[(LEVEL, "a-remote", 20), (REGION, "r", 1)]);
        let p = plan(&b, &local, &remote, DEV);
        // The peer's world is newer: ours moves aside and theirs comes in whole.
        assert_eq!(p.rename, [(".minecraft/saves/W".to_string(), ".minecraft/saves/W (Desktop)".to_string())]);
        assert_eq!(p.take.len(), 2);
        assert_eq!(p.conflicts, ["W"]);

        // Seen from the peer, the same choice: its world keeps the name, ours arrives beside it.
        let mirror = plan(&b, &remote, &local, Devices { local: "Laptop", remote: "Desktop" });
        assert!(mirror.rename.is_empty());
        assert!(mirror.take.iter().all(|(_, to)| to.starts_with(".minecraft/saves/W (Desktop)/")));
    }

    #[test]
    fn file_conflict_newer_wins_and_change_beats_delete() {
        let b = base(&[(".minecraft/servers.dat", "s"), (".minecraft/mods/a.jar", "a")]);
        let local = files(&[(".minecraft/servers.dat", "mine", 5)]);
        let remote = files(&[(".minecraft/servers.dat", "theirs", 9), (".minecraft/mods/a.jar", "a2", 3)]);
        let p = plan(&b, &local, &remote, DEV);
        assert!(p.take.contains(&(".minecraft/servers.dat".into(), ".minecraft/servers.dat".into())));
        // We deleted a.jar, they updated it: the update wins.
        assert!(p.take.contains(&(".minecraft/mods/a.jar".into(), ".minecraft/mods/a.jar".into())));
        assert_eq!(p.conflicts.len(), 2);
    }
}

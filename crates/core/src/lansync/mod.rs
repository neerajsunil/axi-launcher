//! LAN sync: keeps an instance's content, worlds and server list the same on every
//! computer on the network where Axi Launcher runs with the same account.
//!
//! Launchers find each other with UDP beacons ([`discovery`]) and only ever pull: each one
//! serves its synced instances read-only ([`connection`]) and applies a peer's changes to
//! its own copy ([`pull`]) after a three-way merge ([`merge`]). Mods and packs installed
//! from Modrinth or CurseForge are downloaded again rather than copied.

pub mod connection;
pub mod discovery;
mod identity;
pub mod manifest;
pub mod merge;
pub mod protocol;
mod pull;
pub mod rules;

use serde::{Deserialize, Serialize};

pub use connection::{Connection, Host, serve};
pub use discovery::{Advert, Beacon, Discovery};
pub use identity::{SyncIdentity, random_id};
pub use manifest::{Manifest, SyncedMeta};
pub use pull::{PullReport, pull};

/// A computer running the launcher.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Random, made once per launcher data folder.
    pub id: String,
    pub name: String,
}

/// This computer's name as other launchers show it.
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .map(|n| n.trim().to_string())
        .ok()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "Another computer".into())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::instance::{GameSetup, Instance, InstanceManager};
    use crate::{Error, Result};

    struct TestHost {
        device: Device,
        instance: Instance,
        changed: Mutex<Vec<String>>,
    }

    impl Host for TestHost {
        fn identity(&self) -> Option<SyncIdentity> {
            Some(SyncIdentity::new("", "Steve", false))
        }
        fn device(&self) -> Device {
            self.device.clone()
        }
        fn shared_instance(&self, sync_id: &str) -> Result<Instance> {
            let mut instance = self.instance.clone();
            instance.reload()?;
            if instance.synced() != Some(sync_id) {
                return Err(Error::msg("not shared"));
            }
            Ok(instance)
        }
        fn changed(&self, _: &Device, sync_id: &str) {
            self.changed.lock().unwrap().push(sync_id.into());
        }
    }

    fn write(instance: &Instance, path: &str, text: &str) {
        let full = rules::on_disk(&instance.dir, path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }

    fn read(instance: &Instance, path: &str) -> Option<String> {
        fs::read_to_string(rules::on_disk(&instance.dir, path)).ok()
    }

    #[test]
    fn pulls_over_tcp() {
        let tmp = tempfile::tempdir().unwrap();
        let laptop_dir = InstanceManager::new(tmp.path().join("laptop"));
        let desktop_dir = InstanceManager::new(tmp.path().join("desktop"));
        let mut source = laptop_dir.create("Survival", "", GameSetup::vanilla("1.21.10")).unwrap();
        source.config.sync_id = Some("abc".into());
        source.config.lan_sync = true;
        source.save().unwrap();
        write(&source, ".minecraft/saves/W/level.dat", "day 1");
        write(&source, ".minecraft/servers.dat", "hypixel");
        write(&source, ".minecraft/options.txt", "fov:110");

        let host = Arc::new(TestHost {
            device: Device { id: "laptop".into(), name: "Laptop".into() },
            instance: source.clone(),
            changed: Mutex::default(),
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        serve(listener, host.clone());

        let me = Device { id: "desktop".into(), name: "Desktop".into() };
        let identity = SyncIdentity::new("", "steve", false);
        let mut copy = desktop_dir.create("New", "", GameSetup::vanilla("1.20.1")).unwrap();
        let mut conn = Connection::open(addr, &identity, &me).unwrap();
        assert_eq!(conn.peer.name, "Laptop");

        let report = pull(&mut conn, &me, &mut copy, "abc", None, &|_, _| {}).unwrap();
        assert_eq!(report.changed, 2);
        assert!(report.meta_changed);
        assert_eq!(copy.config.game_version, "1.21.10");
        assert_eq!(copy.config.name, "Survival");
        assert_eq!(read(&copy, ".minecraft/saves/W/level.dat").as_deref(), Some("day 1"));
        assert_eq!(read(&copy, ".minecraft/options.txt"), None, "game settings stay local");

        // Laptop plays on; desktop added a server meanwhile. Both changes survive.
        write(&source, ".minecraft/saves/W/level.dat", "day 2");
        write(&copy, ".minecraft/servers.dat", "hypixel+smp");
        let report = pull(&mut conn, &me, &mut copy, "abc", None, &|_, _| {}).unwrap();
        assert!(report.conflicts.is_empty());
        assert_eq!(read(&copy, ".minecraft/saves/W/level.dat").as_deref(), Some("day 2"));
        assert_eq!(read(&copy, ".minecraft/servers.dat").as_deref(), Some("hypixel+smp"));

        // Nothing new: nothing changes.
        let report = pull(&mut conn, &me, &mut copy, "abc", None, &|_, _| {}).unwrap();
        assert_eq!(report.changed, 0);

        conn.changed("abc").unwrap();
        assert_eq!(*host.changed.lock().unwrap(), ["abc"]);
        assert!(conn.manifest("other").is_err());
    }

    #[test]
    fn other_accounts_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let instance = InstanceManager::new(tmp.path()).create("A", "", GameSetup::vanilla("1.21.10")).unwrap();
        let host = Arc::new(TestHost {
            device: Device { id: "a".into(), name: "A".into() },
            instance,
            changed: Mutex::default(),
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        serve(listener, host);
        let me = Device { id: "b".into(), name: "B".into() };
        let err = Connection::open(addr, &SyncIdentity::new("", "Alex", false), &me).err().unwrap();
        assert_eq!(err.to_string(), "different account");
    }
}

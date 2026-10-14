//! Finding other launchers on the network: every few seconds each one broadcasts a UDP
//! beacon with its synced instances. Whoever hears a beacon answers it directly, so
//! discovery still works when only one of the two computers' broadcasts gets through.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::Device;
use super::protocol::{PORT, VERSION};
use crate::Result;
use crate::loader::ModLoader;

const INTERVAL: Duration = Duration::from_secs(4);
/// A peer not heard from for this long is gone.
pub const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beacon {
    pub version: u32,
    pub device: Device,
    /// [`super::SyncIdentity::tag`] of the signed-in account.
    pub account: String,
    /// TCP port of the launcher's server.
    pub port: u16,
    pub instances: Vec<Advert>,
    /// An answer to another beacon; never answered itself.
    #[serde(default)]
    pub reply: bool,
}

/// A synced instance as other launchers see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advert {
    pub sync_id: String,
    pub name: String,
    pub game_version: String,
    pub loader: ModLoader,
    pub loader_version: String,
    /// Being played (or synced) on that computer right now.
    pub busy: bool,
}

/// Running discovery; dropping it does not stop it (it lives as long as the program).
#[derive(Clone)]
pub struct Discovery {
    wake: Sender<()>,
}

impl Discovery {
    /// Sends a beacon now instead of waiting for the next one, e.g. after a game starts.
    pub fn announce(&self) {
        let _ = self.wake.send(());
    }
}

/// Starts broadcasting `beacon()` (nothing while it is `None`) and reports every beacon
/// heard from another launcher, with the address it came from.
pub fn start(
    beacon: impl Fn() -> Option<Beacon> + Send + Sync + 'static,
    heard: impl Fn(Beacon, SocketAddr) + Send + Sync + 'static,
) -> Result<Discovery> {
    // Another launcher on this computer may hold the port; then only answers reach us.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PORT)).or_else(|_| UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
    socket.set_broadcast(true)?;
    let socket = Arc::new(socket);
    let beacon = Arc::new(beacon);
    let (wake, woken) = mpsc::channel();

    {
        let (socket, beacon) = (socket.clone(), beacon.clone());
        thread::spawn(move || {
            loop {
                if let Some(bytes) = beacon().and_then(|b| serde_json::to_vec(&b).ok()) {
                    let _ = socket.send_to(&bytes, (Ipv4Addr::BROADCAST, PORT));
                }
                if let Err(RecvTimeoutError::Disconnected) = woken.recv_timeout(INTERVAL) {
                    // Nobody can wake us early any more; keep the steady pace.
                    thread::sleep(INTERVAL);
                }
            }
        });
    }

    thread::spawn(move || {
        let mut buf = vec![0; 64 * 1024];
        let mut answered: HashMap<String, Instant> = HashMap::new();
        loop {
            let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
            let Ok(heard_beacon) = serde_json::from_slice::<Beacon>(&buf[..n]) else { continue };
            let Some(mine) = beacon() else { continue };
            if heard_beacon.version != VERSION || heard_beacon.device.id == mine.device.id {
                continue;
            }
            let due = answered.get(&heard_beacon.device.id).is_none_or(|t| t.elapsed() >= INTERVAL);
            if !heard_beacon.reply && due {
                answered.insert(heard_beacon.device.id.clone(), Instant::now());
                if let Ok(bytes) = serde_json::to_vec(&Beacon { reply: true, ..mine }) {
                    let _ = socket.send_to(&bytes, from);
                }
            }
            heard(heard_beacon, from);
        }
    });
    Ok(Discovery { wake })
}

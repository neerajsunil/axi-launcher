//! LAN sync in the app: the server and discovery, the peers heard from, and syncing an
//! instance when it launches, after it exits and when asked.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use axi_core::instance::{GameSetup, Instance, unique_name};
use axi_core::lansync::discovery::{self, TIMEOUT};
use axi_core::lansync::protocol::{PORT, VERSION};
use axi_core::lansync::{Advert, Beacon, Connection, Device, Discovery, Host, SyncIdentity, pull, random_id, serve};
use axi_core::mods::ModApi;
use axi_core::util::now_unix;
use axi_core::{Error, Result};

use crate::format;
use crate::state::AppState;

/// Another launcher on the network.
#[derive(Clone)]
pub struct Peer {
    pub device: Device,
    /// Where its server listens.
    pub addr: SocketAddr,
    pub account: String,
    pub instances: Vec<Advert>,
    pub seen: Instant,
}

/// How the last sync of an instance went.
#[derive(Default)]
struct Status {
    /// Unix seconds of the last sync that reached a peer.
    at: Option<u64>,
    with: String,
    error: String,
}

/// Why an instance is being synced; decides what the user is told.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Launch,
    Exit,
    Manual,
    /// Started without the user asking (a peer had changes, sync was just turned on).
    Background,
}

pub struct Lan {
    pub device: Device,
    port: AtomicU16,
    peers: Mutex<HashMap<String, Peer>>,
    /// Instance ids being synced right now.
    syncing: Mutex<HashSet<String>>,
    status: Mutex<HashMap<String, Status>>,
    discovery: OnceLock<Discovery>,
    started: Instant,
}

impl Lan {
    pub fn new(device_id: String) -> Self {
        Self {
            device: Device { id: device_id, name: axi_core::lansync::computer_name() },
            port: AtomicU16::new(0),
            peers: Mutex::default(),
            syncing: Mutex::default(),
            status: Mutex::default(),
            discovery: OnceLock::new(),
            started: Instant::now(),
        }
    }

    /// Lets other launchers see a change (a game started, sync turned on) right away.
    pub fn announce(&self) {
        if let Some(d) = self.discovery.get() {
            d.announce();
        }
    }

    fn is_syncing(&self, id: &str) -> bool {
        self.syncing.lock().unwrap().contains(id)
    }
}

/// Starts serving synced instances and looking for other launchers.
pub fn start(state: &Arc<AppState>) {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, PORT)).or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)));
    match listener {
        Ok(listener) => {
            let port = listener.local_addr().map_or(0, |a| a.port());
            state.lan.port.store(port, Ordering::Relaxed);
            serve(listener, Arc::new(AppHost(state.clone())));
        }
        Err(e) => return eprintln!("lan sync is off: {e}"),
    }

    let beacon = {
        let state = state.clone();
        move || beacon(&state)
    };
    let heard = {
        let state = state.clone();
        move |b: Beacon, from: SocketAddr| heard(&state, b, from)
    };
    match discovery::start(beacon, heard) {
        Ok(d) => {
            let _ = state.lan.discovery.set(d);
        }
        Err(e) => eprintln!("lan discovery is off: {e}"),
    }

    // Forget launchers that went quiet.
    let state = state.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(5));
            let gone = {
                let mut peers = state.lan.peers.lock().unwrap();
                let before = peers.len();
                peers.retain(|_, p| p.seen.elapsed() < TIMEOUT);
                peers.len() != before
            };
            if gone {
                state.mark_dirty();
            }
        }
    });
}

/// The account sync works for: whoever is selected on the Accounts page.
fn identity(state: &AppState) -> Option<SyncIdentity> {
    let accounts = state.accounts();
    let account = accounts.store.active()?;
    Some(SyncIdentity::new(&account.uuid, &account.username, account.is_microsoft()))
}

fn beacon(state: &AppState) -> Option<Beacon> {
    let account = identity(state)?.tag();
    let synced: Vec<(String, Advert)> = state
        .instances
        .lock()
        .unwrap()
        .iter()
        .filter_map(|i| {
            let c = &i.config;
            let advert = Advert {
                sync_id: i.synced()?.to_string(),
                name: c.name.clone(),
                game_version: c.game_version.clone(),
                loader: c.loader,
                loader_version: c.loader_version.clone(),
                busy: false,
            };
            Some((i.id.clone(), advert))
        })
        .collect();
    let instances = synced
        .into_iter()
        .map(|(id, advert)| Advert { busy: state.is_active(&id) || state.lan.is_syncing(&id), ..advert })
        .collect();
    Some(Beacon {
        version: VERSION,
        device: state.lan.device.clone(),
        account,
        port: state.lan.port.load(Ordering::Relaxed),
        instances,
        reply: false,
    })
}

fn heard(state: &AppState, beacon: Beacon, from: SocketAddr) {
    let addr = SocketAddr::new(from.ip(), beacon.port);
    let mut peers = state.lan.peers.lock().unwrap();
    let changed = peers
        .get(&beacon.device.id)
        .is_none_or(|p| p.instances != beacon.instances || p.account != beacon.account || p.device != beacon.device);
    peers.insert(
        beacon.device.id.clone(),
        Peer { device: beacon.device, addr, account: beacon.account, instances: beacon.instances, seen: Instant::now() },
    );
    drop(peers);
    if changed {
        state.mark_dirty();
    }
}

/// Live peers signed in to `identity` that have `sync_id`.
fn peers_with(state: &AppState, identity: &SyncIdentity, sync_id: &str) -> Vec<(Peer, bool)> {
    let tag = identity.tag();
    state
        .lan
        .peers
        .lock()
        .unwrap()
        .values()
        .filter(|p| p.account == tag && p.seen.elapsed() < TIMEOUT)
        .filter_map(|p| Some((p.clone(), p.instances.iter().find(|a| a.sync_id == sync_id)?.busy)))
        .collect()
}

/// Pulls changes to instance `id` from every peer that has it. Blocks; call it off the UI thread.
pub fn sync(state: &Arc<AppState>, id: &str, reason: Reason) {
    sync_from(state, id, reason, None);
}

fn sync_from(state: &Arc<AppState>, id: &str, reason: Reason, only: Option<&str>) {
    let Some(mut instance) = state.instance(id) else { return };
    let Some(sync_id) = instance.synced().map(str::to_string) else { return };
    let name = instance.config.name.clone();
    let Some(identity) = identity(state) else {
        if reason == Reason::Manual {
            state.error("Sign in to sync");
        }
        return;
    };
    if !state.lan.syncing.lock().unwrap().insert(id.to_string()) {
        return;
    }
    state.lan.announce();

    let mut peers = peers_with(state, &identity, &sync_id);
    // Right after the launcher opens, the other computers may not have been heard from yet.
    // Ask them to answer rather than launch an out-of-date copy.
    if reason == Reason::Launch && peers.is_empty() && state.lan.started.elapsed() < TIMEOUT {
        state.set_task(id, Some(("Syncing...".to_string(), 0.0)));
        let deadline = Instant::now() + Duration::from_secs(3);
        while peers.is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(250));
            peers = peers_with(state, &identity, &sync_id);
        }
    }
    if let Some(only) = only {
        peers.retain(|(p, _)| p.device.id == only);
    }
    let (busy, idle): (Vec<_>, Vec<_>) = peers.into_iter().partition(|(_, busy)| *busy);
    if reason == Reason::Launch && !busy.is_empty() {
        let names = busy.iter().map(|(p, _)| p.device.name.as_str()).collect::<Vec<_>>().join(", ");
        state.error(format!("\"{name}\" is open on {names}"));
    }

    let key = state.settings().curseforge_key();
    let api = ModApi { client: &state.client, curseforge_key: &key };
    let me = state.lan.device.clone();
    let task = |text: String, p: f32| state.set_task(id, Some((text, p)));
    let (mut synced_with, mut errors, mut conflicts, mut changed) = (Vec::new(), Vec::new(), Vec::new(), 0);
    for (peer, _) in &idle {
        let result = Connection::open(peer.addr, &identity, &me)
            .and_then(|mut conn| pull(&mut conn, &me, &mut instance, &sync_id, Some(&api), &task));
        match result {
            Ok(report) => {
                synced_with.push(peer.device.name.clone());
                changed += report.changed + usize::from(report.meta_changed);
                conflicts.extend(report.conflicts);
            }
            Err(e) => errors.push(format!("{}: {e}", peer.device.name)),
        }
    }
    // A launch keeps its own progress going.
    state.set_task(id, (reason == Reason::Launch).then(|| ("Preparing...".to_string(), 0.0)));

    {
        let mut status = state.lan.status.lock().unwrap();
        let entry = status.entry(id.to_string()).or_default();
        if !synced_with.is_empty() {
            entry.at = Some(now_unix());
            entry.with = synced_with.join(", ");
        }
        entry.error = errors.join("; ");
    }
    if !conflicts.is_empty() {
        conflicts.sort();
        conflicts.dedup();
        state.info(format!("\"{name}\" changed on both computers, so we kept both copies"));
    } else if !errors.is_empty() && reason != Reason::Background {
        eprintln!("sync of {name} failed: {}", errors.join("; "));
        state.error(format!("Couldn't sync \"{name}\""));
    } else if reason == Reason::Manual && idle.is_empty() && busy.is_empty() {
        state.info("Other computer offline");
    } else if reason == Reason::Manual && changed == 0 && !synced_with.is_empty() {
        state.info(format!("\"{name}\" is already up to date"));
    } else if changed > 0 && matches!(reason, Reason::Manual | Reason::Background) {
        state.info(format!("Synced \"{name}\""));
    }

    state.lan.syncing.lock().unwrap().remove(id);
    state.reload_instances();
    state.lan.announce();
}

/// After a game exits: pull anything new, then tell peers to pull this computer's changes.
pub fn after_exit(state: &Arc<AppState>, id: &str) {
    sync(state, id, Reason::Exit);
    let Some(instance) = state.instance(id) else { return };
    let (Some(sync_id), Some(identity)) = (instance.synced(), identity(state)) else { return };
    let me = state.lan.device.clone();
    for (peer, _) in peers_with(state, &identity, sync_id) {
        if let Err(e) = Connection::open(peer.addr, &identity, &me).and_then(|mut c| c.changed(sync_id)) {
            eprintln!("could not reach {}: {e}", peer.device.name);
        }
    }
}

/// Turns LAN sync on or off for instance `id`.
pub fn set_enabled(state: &Arc<AppState>, id: &str, on: bool) {
    if state.is_active(id) {
        return state.error("Close the game first");
    }
    let result = state.manager.get(id).and_then(|mut instance| {
        if on && instance.config.sync_id.is_none() {
            instance.config.sync_id = Some(random_id());
        }
        instance.config.lan_sync = on;
        instance.save()
    });
    if let Err(e) = result {
        return { eprintln!("could not change lan sync: {e}"); state.error("Couldn't change sync") };
    }
    state.reload_instances();
    state.lan.announce();
    if on {
        let (state, id) = (state.clone(), id.to_string());
        thread::spawn(move || sync(&state, &id, Reason::Background));
    }
}

/// A synced instance on another computer that isn't here yet.
pub struct Available {
    pub advert: Advert,
    pub device: String,
}

pub fn available(state: &AppState) -> Vec<Available> {
    let Some(identity) = identity(state) else { return Vec::new() };
    let tag = identity.tag();
    let known: HashSet<String> = state.instances.lock().unwrap().iter().filter_map(|i| i.config.sync_id.clone()).collect();
    let mut seen = HashSet::new();
    let mut list: Vec<Available> = state
        .lan
        .peers
        .lock()
        .unwrap()
        .values()
        .filter(|p| p.account == tag && p.seen.elapsed() < TIMEOUT)
        .flat_map(|p| p.instances.iter().map(|a| Available { advert: a.clone(), device: p.device.name.clone() }))
        .filter(|a| !known.contains(&a.advert.sync_id) && seen.insert(a.advert.sync_id.clone()))
        .collect();
    list.sort_by_key(|a| a.advert.name.to_lowercase());
    list
}

/// Makes a local copy of a synced instance from another computer and fills it.
pub fn add(state: &Arc<AppState>, sync_id: &str) {
    let Some(found) = available(state).into_iter().find(|a| a.advert.sync_id == sync_id) else {
        return state.error("Other computer offline");
    };
    let a = found.advert;
    let name = unique_name(&state.manager.names(), &a.name);
    let game = GameSetup { version: a.game_version.clone(), loader: a.loader, loader_version: a.loader_version.clone() };
    let created = state.manager.create(&name, "", game).and_then(|mut instance| {
        instance.config.sync_id = Some(a.sync_id.clone());
        instance.config.lan_sync = true;
        instance.save().map(|()| instance)
    });
    let instance = match created {
        Ok(instance) => instance,
        Err(e) => return state.error(format!("Couldn't add \"{}\": {e}", a.name)),
    };
    state.reload_instances();
    state.info(format!("Adding \"{}\"...", a.name));
    let state = state.clone();
    thread::spawn(move || sync(&state, &instance.id, Reason::Manual));
}

/// How an instance's sync is doing, for the cloud's color.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// No other computer with it is on.
    Offline = 0,
    Ready = 1,
    Failed = 2,
    Syncing = 3,
}

/// The instance's sync state and a short label for it.
pub fn status(state: &AppState, instance: &Instance) -> (SyncState, String) {
    let Some(sync_id) = instance.synced() else { return (SyncState::Offline, String::new()) };
    if state.lan.is_syncing(&instance.id) {
        return (SyncState::Syncing, "Syncing".into());
    }
    let status = state.lan.status.lock().unwrap();
    let status = status.get(&instance.id);
    if status.is_some_and(|s| !s.error.is_empty()) {
        return (SyncState::Failed, "Sync failed".into());
    }
    let online = identity(state).is_some_and(|identity| !peers_with(state, &identity, sync_id).is_empty());
    let state = if online { SyncState::Ready } else { SyncState::Offline };
    match status.and_then(|s| s.at) {
        Some(at) => (state, format!("Synced {}", format::time_ago(Some(at)).to_lowercase())),
        None if online => (state, "Ready to sync".into()),
        None => (state, "Other computer offline".into()),
    }
}

/// Serves this launcher's synced instances to peers.
struct AppHost(Arc<AppState>);

impl AppHost {
    fn local(&self, sync_id: &str) -> Option<Instance> {
        self.0.instances.lock().unwrap().iter().find(|i| i.synced() == Some(sync_id)).cloned()
    }
}

impl Host for AppHost {
    fn identity(&self) -> Option<SyncIdentity> {
        identity(&self.0)
    }

    fn device(&self) -> Device {
        self.0.lan.device.clone()
    }

    fn shared_instance(&self, sync_id: &str) -> Result<Instance> {
        let mut instance = self.local(sync_id).ok_or_else(|| Error::msg("that instance isn't synced here"))?;
        if self.0.is_active(&instance.id) || self.0.lan.is_syncing(&instance.id) {
            return Err(Error::msg(format!("\"{}\" is in use on {}", instance.config.name, self.0.lan.device.name)));
        }
        instance.reload()?;
        Ok(instance)
    }

    fn changed(&self, from: &Device, sync_id: &str) {
        let Some(instance) = self.local(sync_id) else { return };
        // Changes wait while the game runs here; they come in on the next launch or exit.
        if self.0.is_active(&instance.id) {
            return;
        }
        let (state, from) = (self.0.clone(), from.id.clone());
        thread::spawn(move || sync_from(&state, &instance.id, Reason::Background, Some(&from)));
    }
}

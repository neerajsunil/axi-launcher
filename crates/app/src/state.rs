//! Shared app state. Background threads change it and mark it dirty;
//! `sync.rs` pushes the changes into the UI on the main thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axi_core::account::{AccountStore, StoredAccount};
use axi_core::instance::{Instance, InstanceManager};
use axi_core::launch::GameProcess;
use axi_core::loader::LoaderVersion;
use axi_core::meta::VersionEntry;
use axi_core::mods::{ContentKind, InstalledContent, ModHit, ModSource, ProjectDetails, ProjectVersion};
use axi_core::net::Client;
use axi_core::paths::Paths;
use axi_core::settings::GlobalSettings;

/// Lines of game output kept in memory per instance.
pub const LOG_CAPACITY: usize = 1000;

/// Runtime state of one instance (preparing, running, output).
#[derive(Default)]
pub struct Session {
    /// Download/prepare progress while a launch is being prepared.
    pub task: Option<(String, f32)>,
    pub game: Option<Arc<GameProcess>>,
    /// Set by Stop so the exit isn't reported as a crash.
    pub stop_requested: bool,
    pub logs: VecDeque<String>,
    /// Total lines ever pushed, so the UI can tell which ones are new.
    pub log_total: u64,
}

pub struct VersionList {
    pub entries: Vec<VersionEntry>,
    pub loading: bool,
    pub error: String,
}

/// Loader versions for the loader picker, keyed by "<loader index>|<game version>".
#[derive(Default)]
pub struct LoaderList {
    pub key: String,
    pub loading: bool,
    pub error: String,
    /// Bumped per query so late answers to older queries are ignored.
    pub generation: u64,
    pub cache: HashMap<String, Vec<LoaderVersion>>,
}

impl LoaderList {
    pub fn current(&self) -> &[LoaderVersion] {
        self.cache.get(&self.key).map_or(&[], Vec::as_slice)
    }
}

/// Installed content of one kind in the instance shown in the Content tab.
#[derive(Default)]
pub struct ModsView {
    pub instance: String,
    pub kind: ContentKind,
    pub items: Vec<InstalledContent>,
    /// Iris or Oculus is installed (only checked for shaders).
    pub shader_loader: bool,
    pub filter: String,
    /// File names with a newer version available.
    pub updates: HashSet<String>,
    pub loading: bool,
    pub checking: bool,
}

/// The version picker for one installed project.
#[derive(Default)]
pub struct VersionPicker {
    pub instance: String,
    pub kind: ContentKind,
    pub title: String,
    pub source: ModSource,
    pub project_id: String,
    /// File id of the installed version.
    pub current: String,
    pub items: Vec<ProjectVersion>,
    pub show_incompatible: bool,
    /// How many rows the dialog shows; grows as it scrolls.
    pub limit: usize,
    pub loading: bool,
    pub error: String,
    /// Bumped per opening so a late answer for another project is ignored.
    pub generation: u64,
}

/// Search results on a browse page.
#[derive(Default)]
pub struct Browse {
    /// Empty when browsing modpacks.
    pub instance: String,
    pub kind: ContentKind,
    pub source: ModSource,
    pub text: String,
    pub hits: Vec<ModHit>,
    /// Results requested so far (hits can be fewer after dropping duplicates).
    pub fetched: u32,
    pub total: u32,
    pub searching: bool,
    pub error: String,
    pub generation: u64,
    /// Project ids queued or being installed.
    pub installing: HashSet<String>,
}


/// The page of one Modrinth project, opened from search results.
#[derive(Default)]
pub struct DetailsView {
    pub project_id: String,
    /// Where versions get installed; empty for modpacks.
    pub instance: String,
    pub kind: ContentKind,
    pub details: Option<ProjectDetails>,
    pub versions: Vec<ProjectVersion>,
    /// How many versions the page lists; grows with "Show more".
    pub version_limit: usize,
    pub loading: bool,
    pub error: String,
    /// Bumped per opening so a late answer for another project is ignored.
    pub generation: u64,
}

/// Saved accounts plus which ones are refreshing their sign-in.
#[derive(Default)]
pub struct Accounts {
    pub store: AccountStore,
    pub refreshing: HashSet<String>,
}

/// A Microsoft sign-in in progress, shown in the sign-in dialog.
#[derive(Default)]
pub struct Login {
    /// Bumped on every start and cancel so threads of an old attempt stop reporting.
    pub generation: u64,
    pub code: String,
    pub link: String,
    pub status: String,
    pub error: String,
    pub busy: bool,
    /// Set on success; the sync closes the dialog.
    pub finished: bool,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
}

pub struct AppState {
    pub paths: Paths,
    pub client: Client,
    pub manager: InstanceManager,
    pub settings: Mutex<GlobalSettings>,
    pub accounts: Mutex<Accounts>,
    pub login: Mutex<Login>,
    pub instances: Mutex<Vec<Instance>>,
    pub versions: Mutex<VersionList>,
    pub sessions: Mutex<HashMap<String, Session>>,
    pub search: Mutex<String>,
    pub toast: Mutex<Option<Toast>>,
    pub loaders: Mutex<LoaderList>,
    pub mods: Mutex<ModsView>,
    pub browse: Mutex<Browse>,
    pub details: Mutex<DetailsView>,
    pub picker: Mutex<VersionPicker>,
    /// Status of a Java download started from the Java picker.
    pub java_task: Mutex<Option<(String, f32)>>,
    /// What the Java picker's Download tab lists.
    pub java_releases: Mutex<Vec<axi_core::java::JavaRelease>>,
    /// Held while content is installed so two installs never write `content.toml` at once.
    pub mods_lock: Mutex<()>,
    /// Held while a modpack installs; others wait their turn.
    pub packs_lock: Mutex<()>,
    pub icons: crate::icons::IconStore,
    pub heads: crate::heads::HeadStore,
    /// Modpack instances whose pack logo was already looked up this run.
    pub pack_icons: Mutex<HashSet<String>>,
    dirty: AtomicBool,
}

impl AppState {
    pub fn load(paths: Paths) -> anyhow::Result<Self> {
        let settings = GlobalSettings::load(&paths.settings_file()).unwrap_or_else(|e| {
            eprintln!("settings.toml is invalid ({e}); using defaults");
            GlobalSettings::default()
        });
        let accounts = load_accounts(&paths);
        let manager = InstanceManager::new(paths.instances_dir());
        let instances = manager.list()?;
        let icons = crate::icons::IconStore::new(paths.cache_dir().join("icons"));
        let heads = crate::heads::HeadStore::new(paths.cache_dir().join("heads"));
        Ok(Self {
            client: axi_core::net::http_client()?,
            paths,
            manager,
            settings: Mutex::new(settings),
            accounts: Mutex::new(Accounts { store: accounts, refreshing: HashSet::new() }),
            login: Mutex::default(),
            instances: Mutex::new(instances),
            versions: Mutex::new(VersionList { entries: Vec::new(), loading: true, error: String::new() }),
            sessions: Mutex::new(HashMap::new()),
            search: Mutex::new(String::new()),
            toast: Mutex::new(None),
            loaders: Mutex::default(),
            mods: Mutex::default(),
            browse: Mutex::default(),
            details: Mutex::default(),
            picker: Mutex::default(),
            packs_lock: Mutex::new(()),
            java_task: Mutex::new(None),
            java_releases: Mutex::default(),
            mods_lock: Mutex::new(()),
            icons,
            heads,
            pack_icons: Mutex::default(),
            dirty: AtomicBool::new(true),
        })
    }

    /// Asks the UI to refresh on its next sync tick.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    pub fn settings(&self) -> MutexGuard<'_, GlobalSettings> {
        self.settings.lock().unwrap()
    }

    pub fn accounts(&self) -> MutexGuard<'_, Accounts> {
        self.accounts.lock().unwrap()
    }

    /// Writes the account list to disk and refreshes the UI.
    pub fn save_accounts(&self, store: &AccountStore) {
        if let Err(e) = store.save(&self.paths.accounts_file()) {
            self.error(format!("Could not save accounts: {e}"));
        }
        self.mark_dirty();
    }

    pub fn sessions(&self) -> MutexGuard<'_, HashMap<String, Session>> {
        self.sessions.lock().unwrap()
    }

    /// Re-reads the instance list from disk.
    pub fn reload_instances(&self) {
        match self.manager.list() {
            Ok(list) => *self.instances.lock().unwrap() = list,
            Err(e) => self.error(format!("Could not read instances: {e}")),
        }
        self.mark_dirty();
    }

    pub fn instance(&self, id: &str) -> Option<Instance> {
        self.instances.lock().unwrap().iter().find(|i| i.id == id).cloned()
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.sessions().get(id).is_some_and(|s| s.task.is_some() || s.game.is_some())
    }

    pub fn set_task(&self, id: &str, task: Option<(String, f32)>) {
        self.sessions().entry(id.to_string()).or_default().task = task;
        self.mark_dirty();
    }

    pub fn push_log(&self, id: &str, line: String) {
        let mut sessions = self.sessions();
        let session = sessions.entry(id.to_string()).or_default();
        // Stack traces are tab-indented; the UI font has no glyph for tabs.
        session.logs.push_back(line.replace('\t', "    "));
        session.log_total += 1;
        while session.logs.len() > LOG_CAPACITY {
            session.logs.pop_front();
        }
    }

    pub fn clear_logs(&self, id: &str) {
        if let Some(session) = self.sessions().get_mut(id) {
            session.logs.clear();
            session.log_total += 1; // forces the UI to resync
        }
    }

    pub fn info(&self, text: impl Into<String>) {
        *self.toast.lock().unwrap() = Some(Toast { text: text.into(), error: false });
        self.mark_dirty();
    }

    pub fn error(&self, text: impl Into<String>) {
        *self.toast.lock().unwrap() = Some(Toast { text: text.into(), error: true });
        self.mark_dirty();
    }
}

/// Reads `accounts.json`; on first run, carries over the offline name from older settings.
fn load_accounts(paths: &Paths) -> AccountStore {
    let file = paths.accounts_file();
    if !file.exists() {
        let mut store = AccountStore::default();
        if let Some(name) = axi_core::settings::legacy_username(&paths.settings_file()) {
            store.upsert(StoredAccount::offline(&name));
            if let Err(e) = store.save(&file) {
                eprintln!("could not save accounts.json: {e}");
            }
        }
        return store;
    }
    AccountStore::load(&file).unwrap_or_else(|e| {
        eprintln!("accounts.json is invalid ({e}); starting with no accounts");
        AccountStore::default()
    })
}

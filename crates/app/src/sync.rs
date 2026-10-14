//! Pushes `AppState` into the UI from a timer on the main thread, so background
//! threads never touch the UI directly.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use axi_core::instance::Instance;
use axi_core::mods::{ContentKind, ProjectVersion};

use crate::convert;
use crate::state::AppState;
use crate::{
    AccountItem, AccountsApi, LanInstanceItem, DescriptionBlock, LinkItem, AppApi, DialogKind, InstanceItem, InstancesApi, LoaderVersionItem, LoadersApi, MainWindow, ModHitItem, ModItem,
    JavaApi, ModVersionItem, ModsApi, Nav, VersionsApi, heads, icons, instance_icons,
};

const TICK: Duration = Duration::from_millis(100);
/// Instances on the home page's "Jump back in" list.
const RECENT: usize = 6;

thread_local! {
    /// Runs a full sync right away; set by `start`.
    static FLUSH: RefCell<Option<Rc<dyn Fn()>>> = RefCell::new(None);
}

/// Pushes the state into the UI now instead of on the next tick. UI callbacks that switch
/// what a view shows call this, so the old content never stays on screen for a frame.
pub fn flush() {
    let run = FLUSH.with(|f| f.borrow().clone());
    if let Some(run) = run {
        run();
    }
}

/// Updates only the rows that changed, appending or trimming the tail, so the UI keeps
/// hover and scroll state (a reset would rebuild every row and jump the scroll view).
pub fn sync_model<T: Clone + PartialEq + 'static>(model: &VecModel<T>, items: Vec<T>) {
    let old = model.row_count();
    if old > 0 && items.is_empty() {
        model.set_vec(items);
        return;
    }
    for (i, item) in items.iter().enumerate().take(old) {
        if model.row_data(i).as_ref() != Some(item) {
            model.set_row_data(i, item.clone());
        }
    }
    if items.len() > old {
        model.extend(items.into_iter().skip(old));
    } else {
        for i in (items.len()..old).rev() {
            model.remove(i);
        }
    }
}

struct LogView {
    model: Rc<VecModel<SharedString>>,
    instance: String,
    total: u64,
}

/// The models the UI's lists are bound to; created once and updated in place.
struct Models {
    accounts: Rc<VecModel<AccountItem>>,
    instances: Rc<VecModel<InstanceItem>>,
    recent: Rc<VecModel<InstanceItem>>,
    lan_available: Rc<VecModel<LanInstanceItem>>,
    mods: Rc<VecModel<ModItem>>,
    results: Rc<VecModel<ModHitItem>>,
    versions: Rc<VecModel<ModVersionItem>>,
    loader_versions: Rc<VecModel<LoaderVersionItem>>,
    loader_labels: Rc<VecModel<SharedString>>,
    /// Decoded images by size and URL, so each row keeps the same `Image`.
    icons: RefCell<HashMap<String, Image>>,
    /// Images asked for during the current sync; the rest are dropped after it.
    icons_used: RefCell<HashSet<String>>,
    details_blocks: Rc<VecModel<DescriptionBlock>>,
    details_links: Rc<VecModel<LinkItem>>,
    details_gallery: Rc<VecModel<LinkItem>>,
    details_versions: Rc<VecModel<ModVersionItem>>,
    /// Player heads by UUID, with the version they were built from.
    heads: RefCell<HashMap<String, (u32, Image)>>,
    /// Instance pictures by file, with the file's modified time when decoded.
    instance_icons: RefCell<HashMap<PathBuf, (SystemTime, Option<Image>)>>,
}

impl Models {
    fn icon(&self, state: &Arc<AppState>, url: Option<&str>) -> Option<Image> {
        self.image(state, url?, icons::LOGO)
    }

    fn image(&self, state: &Arc<AppState>, url: &str, size: u32) -> Option<Image> {
        let key = format!("{size}:{url}");
        self.icons_used.borrow_mut().insert(key.clone());
        if let Some(image) = self.icons.borrow().get(&key) {
            return Some(image.clone());
        }
        let image = Image::from_rgba8(icons::get(state, url, size)?);
        self.icons.borrow_mut().insert(key, image.clone());
        Some(image)
    }

    /// The instance's own picture; starts fetching its modpack logo when it has none.
    fn instance_icon(&self, state: &Arc<AppState>, instance: &Instance) -> Option<Image> {
        let path = instance.icon_file();
        let Ok(modified) = fs::metadata(&path).and_then(|m| m.modified()) else {
            instance_icons::fetch_pack_icon(state, instance);
            return None;
        };
        let mut cache = self.instance_icons.borrow_mut();
        match cache.get(&path) {
            Some((time, image)) if *time == modified => image.clone(),
            _ => {
                let image = instance_icons::load(&path).map(Image::from_rgba8);
                cache.insert(path, (modified, image.clone()));
                image
            }
        }
    }

    /// Frees decoded images no row asked for in this sync (they stay cached on disk).
    fn sweep_icons(&self, state: &AppState) {
        let used = self.icons_used.take();
        self.icons.borrow_mut().retain(|key, _| used.contains(key));
        icons::retain(state, &used);
    }

    fn head(&self, state: &Arc<AppState>, uuid: &str) -> Option<Image> {
        let (version, buffer) = heads::get(state, uuid)?;
        let mut cache = self.heads.borrow_mut();
        match cache.get(uuid) {
            Some((v, image)) if *v == version => Some(image.clone()),
            _ => {
                let image = Image::from_rgba8(buffer);
                cache.insert(uuid.to_string(), (version, image.clone()));
                Some(image)
            }
        }
    }
}

pub fn start(ui: &MainWindow, state: Arc<AppState>) -> Timer {
    let models = Models {
        accounts: Rc::default(),
        instances: Rc::default(),
        recent: Rc::default(),
        lan_available: Rc::default(),
        mods: Rc::default(),
        results: Rc::default(),
        versions: Rc::default(),
        loader_versions: Rc::default(),
        loader_labels: Rc::default(),
        icons: RefCell::default(),
        icons_used: RefCell::default(),
        details_blocks: Rc::default(),
        details_links: Rc::default(),
        details_gallery: Rc::default(),
        details_versions: Rc::default(),
        heads: RefCell::default(),
        instance_icons: RefCell::default(),
    };
    let logs = Rc::new(VecModel::<SharedString>::default());
    let api = ui.global::<InstancesApi>();
    api.set_items(ModelRc::from(models.instances.clone()));
    api.set_recent(ModelRc::from(models.recent.clone()));
    api.set_lan_available(ModelRc::from(models.lan_available.clone()));
    api.set_logs(ModelRc::from(logs.clone()));
    ui.global::<AccountsApi>().set_items(ModelRc::from(models.accounts.clone()));
    let mods = ui.global::<ModsApi>();
    mods.set_installed(ModelRc::from(models.mods.clone()));
    mods.set_results(ModelRc::from(models.results.clone()));
    mods.set_versions(ModelRc::from(models.versions.clone()));
    mods.set_details_blocks(ModelRc::from(models.details_blocks.clone()));
    mods.set_details_links(ModelRc::from(models.details_links.clone()));
    mods.set_details_gallery(ModelRc::from(models.details_gallery.clone()));
    mods.set_details_versions(ModelRc::from(models.details_versions.clone()));
    let loaders = ui.global::<LoadersApi>();
    loaders.set_versions(ModelRc::from(models.loader_versions.clone()));
    loaders.set_labels(ModelRc::from(models.loader_labels.clone()));

    let log_view = RefCell::new(LogView { model: logs, instance: String::new(), total: 0 });
    let weak = ui.as_weak();
    let running = Cell::new(false);
    let sync_all: Rc<dyn Fn()> = Rc::new(move || {
        let Some(ui) = weak.upgrade() else { return };
        // A flush from inside a sync would only repeat it.
        if running.replace(true) {
            return;
        }
        if state.take_dirty() {
            sync_state(&ui, &state, &models);
            sync_accounts(&ui, &state, &models);
            sync_loaders(&ui, &state, &models);
            sync_mods(&ui, &state, &models);
            sync_picker(&ui, &state, &models.versions);
            sync_details(&ui, &state, &models);
            models.sweep_icons(&state);
        }
        sync_logs(&ui, &state, &mut log_view.borrow_mut());
        running.set(false);
    });
    FLUSH.with(|f| *f.borrow_mut() = Some(sync_all.clone()));
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, move || sync_all());
    timer
}

fn sync_state(ui: &MainWindow, state: &Arc<AppState>, models: &Models) {
    let api = ui.global::<InstancesApi>();
    let search = state.search.lock().unwrap().to_lowercase();
    let instances = state.instances.lock().unwrap().clone();
    let selected_id = api.get_selected_id();
    {
        let sessions = state.sessions();
        let mut visible = instances
            .iter()
            .filter(|i: &&Instance| {
                search.is_empty()
                    || i.config.name.to_lowercase().contains(&search)
                    || i.config.game_version.contains(&search)
                    || i.config.group.to_lowercase().contains(&search)
            })
            .collect::<Vec<_>>();
        let item = |i: &Instance| {
            let status = crate::lan::status(state, i);
            convert::instance_item(i, sessions.get(&i.id), models.instance_icon(state, i), status)
        };
        if api.get_sort() == 0 {
            visible.sort_by_key(|i| std::cmp::Reverse(recency(i)));
        }
        sync_model(&models.instances, visible.into_iter().map(item).collect());

        // Played ones by when, then never-played ones by when they were made.
        let mut recent: Vec<&Instance> = instances.iter().collect();
        recent.sort_by_key(|i| std::cmp::Reverse(recency(i)));
        sync_model(&models.recent, recent.into_iter().take(RECENT).map(item).collect());
        if let Some(selected) = instances.iter().find(|i| i.id == selected_id.as_str()) {
            let item = item(selected);
            if api.get_selected() != item {
                api.set_selected(item);
            }
        }
    }
    api.set_total_count(instances.len() as i32);
    let available: Vec<LanInstanceItem> = crate::lan::available(state).into_iter().map(convert::lan_instance_item).collect();
    sync_model(&models.lan_available, available);

    let versions = ui.global::<VersionsApi>();
    {
        let list = state.versions.lock().unwrap();
        versions.set_loading(list.loading);
        versions.set_error(list.error.as_str().into());
    }

    let (java_text, java_progress) = state.java_task.lock().unwrap().clone().unwrap_or_default();
    let java = ui.global::<JavaApi>();
    java.set_busy_text(java_text.into());
    java.set_busy_progress(java_progress);

    sync_activity(ui, state);

    let update = state.update.lock().unwrap().as_ref().map(|p| p.version.clone()).unwrap_or_default();
    ui.global::<AppApi>().set_update_version(update.into());

    if let Some(toast) = state.toast.lock().unwrap().take() {
        let app = ui.global::<AppApi>();
        app.set_toast(toast.text.into());
        app.set_toast_error(toast.error);
    }
}

/// Sort key for "most recent first": played instances by when, ahead of never-played
/// ones by when they were created.
fn recency(instance: &Instance) -> (bool, u64) {
    match instance.config.last_played {
        Some(t) => (true, t),
        None => (false, instance.config.created),
    }
}

/// Counts what runs in the background (game preparation, downloads, Java installs) and
/// describes it, for the activity indicator in the navigation rail.
fn sync_activity(ui: &MainWindow, state: &AppState) {
    let tasks: Vec<(String, String, f32)> = state
        .sessions()
        .iter()
        .filter_map(|(id, s)| s.task.as_ref().map(|(status, progress)| (id.clone(), status.clone(), *progress)))
        .collect();
    let mut lines: Vec<String> = tasks
        .into_iter()
        .map(|(id, status, progress)| {
            let name = state.instance(&id).map_or_else(|| "Instance".to_string(), |i| i.config.name);
            format!("{name}: {status} ({:.0}%)", progress * 100.0)
        })
        .collect();
    let installing = state.browse.lock().unwrap().installing.len();
    if installing > 0 {
        lines.push(if installing == 1 { "Downloading 1 project".into() } else { format!("Downloading {installing} projects") });
    }
    if let Some((text, _)) = state.java_task.lock().unwrap().as_ref() {
        lines.push(text.clone());
    }
    let app = ui.global::<AppApi>();
    app.set_tasks(lines.len() as i32);
    app.set_task_text(lines.join("
").into());
}

fn sync_accounts(ui: &MainWindow, state: &Arc<AppState>, models: &Models) {
    let api = ui.global::<AccountsApi>();
    {
        let accounts = state.accounts();
        let active = accounts.store.active.as_deref();
        let rows = accounts
            .store
            .accounts
            .iter()
            .map(|a| {
                let head = if a.is_microsoft() { models.head(state, &a.uuid) } else { None };
                convert::account_item(a, active == Some(a.uuid.as_str()), accounts.refreshing.contains(&a.uuid), head)
            })
            .collect::<Vec<_>>();
        let current = rows.iter().find(|a| a.active).cloned();
        api.set_has_active(current.is_some());
        let current = current.unwrap_or_default();
        ui.global::<AppApi>().set_player_initial(current.initial.clone());
        if api.get_active() != current {
            api.set_active(current);
        }
        sync_model(&models.accounts, rows);
    }

    let mut login = state.login.lock().unwrap();
    api.set_login_code(login.code.as_str().into());
    api.set_login_status(login.status.as_str().into());
    api.set_login_error(login.error.as_str().into());
    api.set_login_busy(login.busy);
    if std::mem::take(&mut login.finished) {
        let nav = ui.global::<Nav>();
        if nav.get_dialog() == DialogKind::MicrosoftLogin {
            nav.set_dialog(DialogKind::None);
        }
    }
}

fn sync_loaders(ui: &MainWindow, state: &AppState, models: &Models) {
    let api = ui.global::<LoadersApi>();
    let list = state.loaders.lock().unwrap();
    let versions = list.current();
    let recommended = axi_core::loader::recommended(versions).map(|v| v.version.clone()).unwrap_or_default();
    sync_model(
        &models.loader_versions,
        versions.iter().map(|v| LoaderVersionItem { version: v.version.as_str().into(), stable: v.stable }).collect(),
    );
    let mut labels: Vec<SharedString> = vec![format!("Recommended ({recommended})").into()];
    labels.extend(versions.iter().map(|v| {
        if v.stable { v.version.as_str().into() } else { format!("{} (beta)", v.version).into() }
    }));
    sync_model(&models.loader_labels, labels);
    api.set_recommended(recommended.into());
    api.set_key(list.key.as_str().into());
    api.set_loading(list.loading);
    api.set_error(list.error.as_str().into());
}

fn sync_mods(ui: &MainWindow, state: &Arc<AppState>, models: &Models) {
    let api = ui.global::<ModsApi>();
    let installed_projects: HashSet<String>;
    let installing = state.browse.lock().unwrap().installing.clone();
    {
        let view = state.mods.lock().unwrap();
        let filter = view.filter.trim().to_lowercase();
        let items = view
            .items
            .iter()
            .filter(|m| filter.is_empty() || m.title.to_lowercase().contains(&filter) || m.file_name.to_lowercase().contains(&filter))
            .map(|m| {
                let icon = models.icon(state, m.entry.as_ref().and_then(|e| e.icon_url.as_deref()));
                let busy = m.entry.as_ref().is_some_and(|e| installing.contains(&e.project_id));
                convert::mod_item(m, &view.updates, busy, icon)
            })
            .collect();
        sync_model(&models.mods, items);
        api.set_installed_count(view.items.len() as i32);
        api.set_disabled_count(view.items.iter().filter(|m| !m.enabled).count() as i32);
        api.set_update_count(view.items.iter().filter(|m| view.updates.contains(&m.file_name)).count() as i32);
        api.set_loading(view.loading);
        api.set_checking_updates(view.checking);
        api.set_shader_loader(view.shader_loader);
        api.set_shown_instance(view.instance.as_str().into());
        api.set_shown_kind(convert::kind_index(view.kind));

        installed_projects = view.items.iter().filter_map(|m| m.entry.as_ref().map(|e| e.project_id.clone())).collect();
    }
    // Modpacks count as installed when an instance was made from them.
    let installed_packs: HashSet<String> = state
        .instances
        .lock()
        .unwrap()
        .iter()
        .filter_map(|i| i.config.pack.as_ref().map(|p| p.project_id.clone()))
        .collect();

    api.set_shader_loader_installing(installing.contains(axi_core::mods::IRIS_MODRINTH_ID));

    let browse = state.browse.lock().unwrap();
    let installed_projects = if browse.kind == ContentKind::Modpack { &installed_packs } else { &installed_projects };
    let results = browse
        .hits
        .iter()
        .map(|h| {
            let icon = models.icon(state, h.icon_url.as_deref());
            let installed = installed_projects.contains(&h.project_id);
            convert::hit_item(h, installed, browse.installing.contains(&h.project_id), icon)
        })
        .collect();
    sync_model(&models.results, results);
    api.set_searching(browse.searching);
    api.set_search_error(browse.error.as_str().into());
    api.set_has_more(browse.fetched < browse.total);
    api.set_curseforge_ready(!state.settings().curseforge_key().is_empty());
}

fn sync_picker(ui: &MainWindow, state: &AppState, items: &VecModel<ModVersionItem>) {
    let api = ui.global::<ModsApi>();
    let picker = state.picker.lock().unwrap();
    let installing = state.browse.lock().unwrap().installing.contains(&picker.project_id);
    let suits = |v: &&ProjectVersion| v.compatible || v.file_id == picker.current;
    // Versions that suit the instance come first, the rest (when shown) after them.
    let shown: Vec<&ProjectVersion> = picker
        .items
        .iter()
        .filter(suits)
        .chain(picker.items.iter().filter(|v| picker.show_incompatible && !suits(v)))
        .collect();
    let rows = shown
        .iter()
        .take(picker.limit)
        .map(|v| convert::version_choice(v, v.file_id == picker.current))
        .collect();
    sync_model(items, rows);
    api.set_versions_title(picker.title.as_str().into());
    api.set_versions_loading(picker.loading);
    api.set_versions_error(picker.error.as_str().into());
    api.set_versions_installing(installing);
    api.set_versions_hidden(picker.items.iter().filter(|v| !suits(v)).count() as i32);
    api.set_versions_more(shown.len() > picker.limit);
    api.set_show_incompatible(picker.show_incompatible);
}

fn sync_details(ui: &MainWindow, state: &Arc<AppState>, models: &Models) {
    let api = ui.global::<ModsApi>();
    let view = state.details.lock().unwrap();

    let hit = state.browse.lock().unwrap().hits.iter().find(|h| h.project_id == view.project_id).cloned();
    let hit_item = hit.as_ref().map(|h| {
        let installed = models.results.iter().find(|r| r.project_id == h.project_id.as_str()).is_some_and(|r| r.installed);
        let installing = state.browse.lock().unwrap().installing.contains(&h.project_id);
        convert::hit_item(h, installed, installing, models.icon(state, h.icon_url.as_deref()))
    });
    let hit_item = hit_item.unwrap_or_default();
    if api.get_details_hit() != hit_item {
        api.set_details_hit(hit_item);
    }

    let empty = Default::default();
    let details = view.details.as_ref().unwrap_or(&empty);
    let icon = models.icon(state, details.icon_url.as_deref());
    let data = convert::project_details(details, hit.as_ref().map_or("", |h| &h.author), hit.as_ref().map_or("", |h| &h.page_url), icon);
    if api.get_details() != data {
        api.set_details(data);
    }

    let blocks = details
        .body
        .iter()
        .map(|block| {
            let image = match block {
                axi_core::mods::description::Block::Image(url) => models.image(state, url, icons::PICTURE),
                _ => None,
            };
            convert::description_block(block, image)
        })
        .collect();
    sync_model(&models.details_blocks, blocks);
    let links = details
        .links
        .iter()
        .map(|(label, url)| LinkItem { label: label.as_str().into(), url: url.as_str().into(), ..Default::default() })
        .collect();
    sync_model(&models.details_links, links);
    let gallery = details
        .gallery
        .iter()
        .map(|g| {
            let image = models.image(state, &g.url, icons::PICTURE);
            LinkItem {
                label: g.title.as_str().into(),
                url: g.url.as_str().into(),
                has_image: image.is_some(),
                image: image.unwrap_or_default(),
            }
        })
        .collect();
    sync_model(&models.details_gallery, gallery);
    let versions = view.versions.iter().take(view.version_limit).map(|v| convert::version_choice(v, false)).collect();
    sync_model(&models.details_versions, versions);
    api.set_details_versions_more(view.versions.len() > view.version_limit);
    api.set_details_loading(view.loading);
    api.set_details_error(view.error.as_str().into());
}

fn sync_logs(ui: &MainWindow, state: &AppState, view: &mut LogView) {
    let selected = ui.global::<InstancesApi>().get_selected_id();
    let sessions = state.sessions();
    let (lines, total) = sessions
        .get(selected.as_str())
        .map_or((None, 0), |s| (Some(&s.logs), s.log_total));

    let switched = view.instance != selected.as_str();
    if !switched && total == view.total {
        return;
    }
    let new = total.saturating_sub(view.total) as usize;
    let all: Vec<SharedString> = lines.into_iter().flatten().map(|l| l.as_str().into()).collect();
    if switched || total < view.total || new > all.len() || new > view.model.row_count() + all.len() {
        view.model.set_vec(all);
    } else {
        for line in &all[all.len() - new..] {
            view.model.push(line.clone());
        }
        let excess = view.model.row_count().saturating_sub(crate::state::LOG_CAPACITY);
        for _ in 0..excess {
            view.model.remove(0);
        }
    }
    view.instance = selected.to_string();
    view.total = total;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(model: &VecModel<i32>) -> Vec<i32> {
        model.iter().collect()
    }

    #[test]
    fn sync_model_appends_updates_and_trims_in_place() {
        let model = VecModel::from(vec![1, 2, 3]);
        sync_model(&model, vec![1, 2, 3, 4, 5]);
        assert_eq!(rows(&model), [1, 2, 3, 4, 5]);
        sync_model(&model, vec![1, 9, 3]);
        assert_eq!(rows(&model), [1, 9, 3]);
        sync_model(&model, vec![]);
        assert!(rows(&model).is_empty());
        sync_model(&model, vec![7]);
        assert_eq!(rows(&model), [7]);
    }
}

//! The browse pages: searching Modrinth / CurseForge for mods, resource packs, shaders
//! (installed into the selected instance) and modpacks (installed as new instances).

use std::sync::Arc;
use std::thread;

use axi_core::loader::ModLoader;
use axi_core::mods::{ContentKind, ModApi, ModSource, SearchQuery};
use slint::ComponentHandle;

use super::mods;
use crate::{convert, sync};
use crate::state::{AppState, DetailsView};
use crate::{MainWindow, ModsApi};

const PAGE_SIZE: u32 = 20;
/// Versions listed on a project page at first, and added per "Show more".
const VERSION_PAGE: usize = 15;

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<ModsApi>();
    {
        let state = state.clone();
        api.on_search(move |id, kind, source, text| {
            let source = convert::source_from_index(source);
            let kind = convert::kind_from_index(kind);
            {
                let mut browse = state.browse.lock().unwrap();
                // A new query keeps the old results on screen until its first page arrives;
                // a different list (instance, kind, source) starts empty.
                if browse.instance != id.as_str() || browse.kind != kind || browse.source != source {
                    browse.hits.clear();
                    browse.fetched = 0;
                    browse.total = 0;
                }
                browse.instance = id.to_string();
                browse.kind = kind;
                browse.source = source;
                browse.text = text.to_string();
                browse.error.clear();
            }
            fetch(&state, true);
            sync::flush();
        });
    }
    {
        let state = state.clone();
        api.on_load_more(move || {
            let more = {
                let browse = state.browse.lock().unwrap();
                !browse.searching && browse.fetched < browse.total
            };
            if more {
                fetch(&state, false);
            }
        });
    }
    {
        let state = state.clone();
        api.on_install(move |id, kind, source, project_id| {
            let source = convert::source_from_index(source);
            let title = state.browse.lock().unwrap().hits.iter().find(|h| h.project_id == project_id.as_str()).map(|h| h.title.clone());
            let job = mods::Job::newest(source, project_id.as_str());
            let kind = convert::kind_from_index(kind);
            mods::install(state.clone(), id.to_string(), kind, vec![job], title.unwrap_or_else(|| "The download".into()));
        });
    }
    {
        let state = state.clone();
        api.on_open_details(move |id, kind, project_id| {
            open_details(&state, &id, convert::kind_from_index(kind), &project_id);
            sync::flush();
        });
    }
    {
        let state = state.clone();
        api.on_details_more_versions(move || {
            state.details.lock().unwrap().version_limit += VERSION_PAGE;
            state.mark_dirty();
        });
    }
    {
        let state = state.clone();
        api.on_details_install_version(move |file_id| {
            let found = {
                let view = state.details.lock().unwrap();
                let title = view.details.as_ref().map_or("The download", |d| d.title.as_str());
                view.versions.iter().find(|v| v.file_id == file_id.as_str()).map(|v| {
                    let job = mods::Job { source: ModSource::Modrinth, project_id: view.project_id.clone(), file_id: Some(v.file_id.clone()) };
                    (view.instance.clone(), view.kind, job, format!("{title} {}", v.version_number))
                })
            };
            if let Some((id, kind, job, label)) = found {
                mods::install(state.clone(), id, kind, vec![job], label);
            }
        });
    }
    {
        let state = state.clone();
        api.on_open_url(move |url| {
            if !url.is_empty() {
                if let Err(e) = open::that_detached(url.as_str()) {
                    { eprintln!("could not open the browser: {e}"); state.error("Couldn't open the browser") };
                }
            }
        });
    }
}

/// Loads the page and the versions of a Modrinth project in the background.
fn open_details(state: &Arc<AppState>, instance: &str, kind: ContentKind, project_id: &str) {
    let generation = {
        let mut view = state.details.lock().unwrap();
        *view = DetailsView {
            project_id: project_id.to_string(),
            instance: if kind == ContentKind::Modpack { String::new() } else { instance.to_string() },
            kind,
            version_limit: VERSION_PAGE,
            loading: true,
            generation: view.generation + 1,
            ..DetailsView::default()
        };
        view.generation
    };
    state.mark_dirty();

    let (game_version, loader) = state
        .instance(instance)
        .filter(|_| kind != ContentKind::Modpack)
        .map_or((String::new(), ModLoader::Vanilla), |i| (i.config.game_version, i.config.loader));
    let (state, project_id) = (state.clone(), project_id.to_string());
    thread::spawn(move || {
        let api = ModApi { client: &state.client, curseforge_key: "" };
        let (details, versions) = rayon::join(
            || api.project_details(&project_id),
            || api.versions(ModSource::Modrinth, kind, &project_id, &game_version, loader),
        );
        let mut view = state.details.lock().unwrap();
        if view.generation != generation {
            return;
        }
        view.loading = false;
        match details {
            Ok(details) => view.details = Some(details),
            Err(e) => view.error = format!("Couldn't load this project: {e}"),
        }
        match versions {
            Ok(versions) => view.versions = versions,
            Err(e) if view.error.is_empty() => view.error = format!("Couldn't load the versions: {e}"),
            Err(_) => {}
        }
        drop(view);
        state.mark_dirty();
    });
}

/// Loads the first page of results for the current search (`fresh`) or the next one.
fn fetch(state: &Arc<AppState>, fresh: bool) {
    let (generation, source, query) = {
        let mut browse = state.browse.lock().unwrap();
        // Modpacks aren't tied to an instance; everything else must suit the selected one.
        let (game_version, loader) = if browse.kind == ContentKind::Modpack {
            (String::new(), ModLoader::Vanilla)
        } else {
            let Some(instance) = state.instance(&browse.instance) else { return };
            (instance.config.game_version, instance.config.loader)
        };
        browse.generation += 1;
        browse.error.clear();
        let curseforge_without_key =
            browse.source == ModSource::CurseForge && state.settings().curseforge_key().is_empty();
        browse.searching = !curseforge_without_key;
        if curseforge_without_key {
            drop(browse);
            state.mark_dirty();
            return; // the page asks for a key instead
        }
        let query = SearchQuery {
            text: browse.text.clone(),
            kind: browse.kind,
            game_version,
            loader,
            offset: if fresh { 0 } else { browse.fetched },
            limit: PAGE_SIZE,
        };
        (browse.generation, browse.source, query)
    };
    state.mark_dirty();

    let state = state.clone();
    thread::spawn(move || {
        let key = state.settings().curseforge_key();
        let api = ModApi { client: &state.client, curseforge_key: &key };
        let result = api.search(source, &query);
        let mut browse = state.browse.lock().unwrap();
        if browse.generation != generation {
            return; // superseded by a newer search
        }
        browse.searching = false;
        match result {
            Ok(page) => {
                browse.fetched = query.offset + page.hits.len() as u32;
                // An empty page means the end, whatever the reported total says.
                browse.total = if page.hits.is_empty() { browse.fetched } else { page.total };
                if fresh {
                    browse.hits = page.hits;
                } else {
                    // Pages can overlap when the index shifts between requests.
                    let seen: std::collections::HashSet<String> = browse.hits.iter().map(|h| h.project_id.clone()).collect();
                    browse.hits.extend(page.hits.into_iter().filter(|h| !seen.contains(&h.project_id)));
                }
            }
            Err(e) => {
                if fresh {
                    browse.hits.clear();
                    browse.fetched = 0;
                    browse.total = 0;
                }
                browse.error = format!("Search failed: {e}");
            }
        }
        drop(browse);
        state.mark_dirty();
    });
}

//! The accounts page: Microsoft sign-in, offline accounts, switching and removing.

use std::sync::Arc;
use std::thread;

use axi_core::account::{Account, StoredAccount, microsoft};
use axi_core::settings::validate_username;
use slint::ComponentHandle;

use crate::state::{AppState, Login};
use crate::{AccountsApi, MainWindow};

pub fn register(ui: &MainWindow, state: &Arc<AppState>) {
    let api = ui.global::<AccountsApi>();
    {
        let state = state.clone();
        api.on_sign_in(move || sign_in(state.clone()));
    }
    {
        let state = state.clone();
        api.on_cancel_sign_in(move || {
            let mut login = state.login.lock().unwrap();
            *login = Login { generation: login.generation + 1, ..Login::default() };
        });
    }
    {
        let state = state.clone();
        api.on_open_sign_in_page(move || {
            let link = state.login.lock().unwrap().link.clone();
            if !link.is_empty() {
                if let Err(e) = open::that_detached(&link) {
                    { eprintln!("could not open the browser: {e}"); state.error("Couldn't open the browser") };
                }
            }
        });
    }
    {
        let state = state.clone();
        api.on_add_offline(move |name| {
            let name = name.trim();
            if let Err(e) = validate_username(name) {
                return state.error(e);
            }
            let store = {
                let mut accounts = state.accounts();
                accounts.store.upsert(StoredAccount::offline(name));
                accounts.store.clone()
            };
            state.save_accounts(&store);
            state.info(format!("Playing as {name}"));
        });
    }
    {
        let state = state.clone();
        api.on_select(move |id| {
            let store = {
                let mut accounts = state.accounts();
                if !accounts.store.set_active(&id) {
                    return;
                }
                accounts.store.clone()
            };
            state.save_accounts(&store);
        });
    }
    {
        let state = state.clone();
        api.on_remove(move |id| {
            let (removed, store) = {
                let mut accounts = state.accounts();
                (accounts.store.remove(&id), accounts.store.clone())
            };
            if let Some(account) = removed {
                state.save_accounts(&store);
                state.info(format!("Removed {}", account.username));
            }
        });
    }
    {
        let state = state.clone();
        api.on_refresh(move |id| {
            let state = state.clone();
            let id = id.to_string();
            thread::spawn(move || match refresh(&state, &id) {
                Ok(account) => state.info(format!("{} is good to go", account.username)),
                Err(e) => state.error(e),
            });
        });
    }
}

/// Runs the device code flow on a background thread, reporting into `state.login`.
fn sign_in(state: Arc<AppState>) {
    let generation = {
        let mut login = state.login.lock().unwrap();
        *login = Login {
            generation: login.generation + 1,
            status: "Contacting Microsoft...".into(),
            busy: true,
            ..Login::default()
        };
        login.generation
    };
    state.mark_dirty();

    thread::spawn(move || {
        // Changes the dialog only while this attempt is still the current one.
        let update = |change: &dyn Fn(&mut Login)| {
            let mut login = state.login.lock().unwrap();
            if login.generation == generation {
                change(&mut login);
                state.mark_dirty();
            }
        };
        let cancelled = || state.login.lock().unwrap().generation != generation;

        let result = (|| {
            let client_id = microsoft::client_id(&state.settings().microsoft_client_id)?;
            let code = microsoft::request_device_code(&state.client, &client_id)?;
            update(&|l| {
                l.code = code.user_code.clone();
                l.link = code.link();
                l.status = "Waiting for you to sign in...".into();
            });
            let Some(token) = microsoft::wait_for_token(&state.client, &client_id, &code, &cancelled)? else {
                return Ok(None);
            };
            update(&|l| l.code.clear());
            microsoft::finish_sign_in(&state.client, token, &|text| update(&|l| l.status = text.into())).map(Some)
        })();

        match result {
            Ok(None) => {}
            Ok(Some(account)) if !cancelled() => {
                let name = account.username.clone();
                let store = {
                    let mut accounts = state.accounts();
                    accounts.store.upsert(account);
                    accounts.store.clone()
                };
                state.save_accounts(&store);
                update(&|l| {
                    l.busy = false;
                    l.finished = true;
                });
                state.info(format!("Welcome, {name}!"));
            }
            Ok(Some(_)) => {}
            Err(e) => update(&|l| {
                l.busy = false;
                l.code.clear();
                l.error = e.to_string();
            }),
        }
    });
}

/// Renews a Microsoft account's tokens and saves them. Blocks on the network.
fn refresh(state: &AppState, id: &str) -> Result<StoredAccount, String> {
    let account = {
        let mut accounts = state.accounts();
        let account = accounts.store.get(id).cloned().ok_or("That account no longer exists")?;
        if !accounts.refreshing.insert(id.to_string()) {
            return Err(format!("{} is already signing in", account.username));
        }
        account
    };
    state.mark_dirty();

    let result = microsoft::client_id(&state.settings().microsoft_client_id)
        .and_then(|client_id| microsoft::refresh(&state.client, &client_id, &account));
    let store = {
        let mut accounts = state.accounts();
        accounts.refreshing.remove(id);
        if let Ok(fresh) = &result {
            accounts.store.replace(fresh.clone());
        }
        accounts.store.clone()
    };
    state.save_accounts(&store);
    result.map_err(|e| format!("Couldn't refresh {}: {e}", account.username))
}

/// The active account, ready to launch with; refreshes Microsoft tokens that expired.
pub fn launch_account(state: &AppState) -> Result<Account, String> {
    let account = state
        .accounts()
        .store
        .active()
        .cloned()
        .ok_or("Add an account on the Accounts page before playing")?;
    match &account.microsoft {
        Some(auth) if auth.needs_refresh() => Ok(refresh(state, &account.uuid)?.launch_identity()),
        _ => Ok(account.launch_identity()),
    }
}

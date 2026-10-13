use std::time::Duration;

use reqwest::blocking::Client;
use serde::de::DeserializeOwned;

use crate::Result;

pub fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("axi-launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()?)
}

pub fn get_json<T: DeserializeOwned>(client: &Client, url: &str) -> Result<T> {
    Ok(client.get(url).send()?.error_for_status()?.json()?)
}

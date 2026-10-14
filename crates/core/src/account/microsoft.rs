//! Microsoft sign-in with the OAuth device code flow:
//! device code → Microsoft token → Xbox Live → XSTS → Minecraft token → profile.
//!
//! Needs an Azure app registration (personal accounts, public client flows on)
//! that Mojang has approved for the Minecraft API.

use std::thread;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::{MicrosoftAuth, StoredAccount};
use crate::util::now_unix;
use crate::{Error, Result};

const DEVICE_CODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const SCOPE: &str = "XboxLive.signin offline_access";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN_URL: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";

/// Azure client ID compiled in through the `AXI_MSA_CLIENT_ID` environment variable.
pub const BUILT_IN_CLIENT_ID: Option<&str> = option_env!("AXI_MSA_CLIENT_ID");

/// The client ID to sign in with: the configured one, else the built-in one.
pub fn client_id(configured: &str) -> Result<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        return Ok(configured.to_string());
    }
    BUILT_IN_CLIENT_ID.filter(|id| !id.is_empty()).map(str::to_string).ok_or_else(|| {
        Error::msg("Add a Microsoft client ID in Settings first.")
    })
}

/// What the user needs to sign in in their browser.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

impl DeviceCode {
    /// The sign-in page, with the code filled in when the page supports it.
    pub fn link(&self) -> String {
        if self.verification_uri.contains("microsoft.com/link") {
            format!("https://www.microsoft.com/link?otc={}", self.user_code)
        } else {
            self.verification_uri.clone()
        }
    }
}

/// Microsoft OAuth tokens, before they are traded for a Minecraft token.
#[derive(Debug, Clone, Deserialize)]
pub struct MicrosoftToken {
    access_token: String,
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct OAuthError {
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Step 1: asks Microsoft for a code the user enters in their browser.
pub fn request_device_code(client: &Client, client_id: &str) -> Result<DeviceCode> {
    let response = client.post(DEVICE_CODE_URL).form(&[("client_id", client_id), ("scope", SCOPE)]).send()?;
    oauth_json(response)
}

/// Step 2: waits until the user has signed in. Returns `None` once `cancelled` returns true.
pub fn wait_for_token(
    client: &Client,
    client_id: &str,
    code: &DeviceCode,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<MicrosoftToken>> {
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = Duration::from_secs(code.interval.max(1));
    loop {
        if !sleep_unless_cancelled(interval, cancelled) {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            return Err(Error::msg("The sign-in code expired. Try again."));
        }
        let form = [("grant_type", DEVICE_GRANT), ("client_id", client_id), ("device_code", &code.device_code)];
        let response = client.post(TOKEN_URL).form(&form).send()?;
        if response.status().is_success() {
            return Ok(Some(response.json()?));
        }
        let error = read_oauth_error(response);
        match error.error.as_str() {
            "authorization_pending" => {}
            "slow_down" => interval += Duration::from_secs(5),
            _ => return Err(oauth_error(error)),
        }
    }
}

/// Step 3: trades the Microsoft token for a Minecraft account. `status` reports each stage.
pub fn finish_sign_in(client: &Client, token: MicrosoftToken, status: &dyn Fn(&str)) -> Result<StoredAccount> {
    status("Signing in to Xbox Live...");
    let xbl = xbox_user_token(client, &token.access_token)?;
    let xsts = xsts_token(client, &xbl.token)?;
    let uhs = xbl.display_claims.xui.first().map(|x| x.uhs.clone()).unwrap_or_default();

    status("Signing in to Minecraft...");
    let response = client
        .post(MC_LOGIN_URL)
        .json(&json!({ "identityToken": format!("XBL3.0 x={uhs};{}", xsts.token) }))
        .send()?;
    let minecraft: MinecraftToken = match response.status() {
        StatusCode::FORBIDDEN => {
            return Err(Error::msg(
                "This client ID isn't approved for Minecraft yet.",
            ));
        }
        StatusCode::TOO_MANY_REQUESTS => return Err(Error::msg("Whoa, too many sign-ins. Try again in a minute.")),
        _ => response.error_for_status()?.json()?,
    };

    status("Loading your profile...");
    let response = client.get(PROFILE_URL).bearer_auth(&minecraft.access_token).send()?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(Error::msg("This account doesn't own Minecraft Java."));
    }
    let profile: Profile = response.error_for_status()?.json()?;

    Ok(StoredAccount {
        uuid: profile.id,
        username: profile.name,
        microsoft: Some(MicrosoftAuth {
            refresh_token: token.refresh_token,
            access_token: minecraft.access_token,
            expires_at: now_unix() + minecraft.expires_in,
        }),
    })
}

/// Renews an account's tokens (and name, if it changed) without asking the user.
pub fn refresh(client: &Client, client_id: &str, account: &StoredAccount) -> Result<StoredAccount> {
    let auth = account.microsoft.as_ref().ok_or_else(|| Error::msg("not a Microsoft account"))?;
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", &auth.refresh_token),
        ("scope", SCOPE),
    ];
    let response = client.post(TOKEN_URL).form(&form).send()?;
    let token: MicrosoftToken = oauth_json(response).map_err(|e| match e {
        Error::Msg(m) => Error::msg(format!("{m} Remove {} and sign in again.", account.username)),
        e => e,
    })?;
    finish_sign_in(client, token, &|_| {})
}

/// Sleeps in short steps; false if cancelled meanwhile.
fn sleep_unless_cancelled(total: Duration, cancelled: &dyn Fn() -> bool) -> bool {
    let step = Duration::from_millis(250);
    let end = Instant::now() + total;
    while Instant::now() < end {
        if cancelled() {
            return false;
        }
        thread::sleep(step.min(end - Instant::now()));
    }
    !cancelled()
}

fn oauth_json<T: DeserializeOwned>(response: Response) -> Result<T> {
    if response.status().is_success() {
        return Ok(response.json()?);
    }
    Err(oauth_error(read_oauth_error(response)))
}

fn read_oauth_error(response: Response) -> OAuthError {
    let status = response.status();
    response.json().unwrap_or_else(|_| OAuthError { error: String::new(), error_description: format!("HTTP {status}") })
}

fn oauth_error(e: OAuthError) -> Error {
    let text = match e.error.as_str() {
        "authorization_declined" | "access_denied" => "Sign-in was cancelled.".to_string(),
        "expired_token" => "The sign-in code expired. Try again.".to_string(),
        "invalid_grant" => "Your Microsoft sign-in has expired.".to_string(),
        // AADSTS700016: no app with this ID takes personal accounts.
        _ if e.error_description.contains("AADSTS700016") => {
            "Microsoft can't find this client ID. Use the Application (client) ID, and let the app accept personal Microsoft accounts.".to_string()
        }
        // AADSTS7000218: the app wants a secret, i.e. public client flows are off.
        _ if e.error_description.contains("AADSTS7000218") => {
            "Turn on \"Allow public client flows\" for your Azure app.".to_string()
        }
        "invalid_client" | "unauthorized_client" => {
            "Your Azure app needs personal accounts and public client flows turned on.".to_string()
        }
        _ => {
            // Descriptions end with trace and correlation IDs; keep the readable part.
            let text = e.error_description.split("Trace ID:").next().unwrap_or_default().trim();
            format!("Microsoft sign-in failed: {}", if text.is_empty() { &e.error } else { text })
        }
    };
    Error::Msg(text)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxToken {
    token: String,
    display_claims: DisplayClaims,
}

#[derive(Debug, Deserialize)]
struct DisplayClaims {
    xui: Vec<Xui>,
}

#[derive(Debug, Deserialize)]
struct Xui {
    uhs: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxError {
    x_err: u64,
}

#[derive(Debug, Deserialize)]
struct MinecraftToken {
    access_token: String,
    expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct Profile {
    id: String,
    name: String,
}

fn xbox_user_token(client: &Client, ms_access_token: &str) -> Result<XboxToken> {
    let body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={ms_access_token}"),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    xbox_request(client, XBL_URL, &body)
}

fn xsts_token(client: &Client, xbl_token: &str) -> Result<XboxToken> {
    let body = json!({
        "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    xbox_request(client, XSTS_URL, &body)
}

fn xbox_request(client: &Client, url: &str, body: &serde_json::Value) -> Result<XboxToken> {
    let response = client.post(url).header("Accept", "application/json").json(body).send()?;
    if response.status() == StatusCode::UNAUTHORIZED {
        let code = response.json::<XboxError>().map_or(0, |e| e.x_err);
        return Err(Error::msg(xbox_error_text(code)));
    }
    Ok(response.error_for_status()?.json()?)
}

fn xbox_error_text(code: u64) -> String {
    match code {
        2148916233 => "This Microsoft account has no Xbox profile. Sign in once at minecraft.net to create one.".into(),
        2148916235 => "Xbox Live is not available in your country.".into(),
        2148916236 | 2148916237 => "This account needs adult verification on the Xbox website.".into(),
        2148916238 => "This is a child account. An adult must add it to a Microsoft family first.".into(),
        _ => format!("Xbox Live refused the sign-in (error {code})."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_code_link_prefills_code() {
        let code: DeviceCode = serde_json::from_str(
            r#"{"device_code":"d","user_code":"ABCD1234","verification_uri":"https://www.microsoft.com/link","expires_in":900,"interval":5,"message":"..."}"#,
        )
        .unwrap();
        assert_eq!(code.link(), "https://www.microsoft.com/link?otc=ABCD1234");
        assert_eq!(code.interval, 5);
    }

    #[test]
    fn parses_xbox_responses() {
        let token: XboxToken =
            serde_json::from_str(r#"{"IssueInstant":"x","Token":"tok","DisplayClaims":{"xui":[{"uhs":"123"}]}}"#)
                .unwrap();
        assert_eq!((token.token.as_str(), token.display_claims.xui[0].uhs.as_str()), ("tok", "123"));
        let error: XboxError = serde_json::from_str(r#"{"Identity":"0","XErr":2148916233,"Message":""}"#).unwrap();
        assert!(xbox_error_text(error.x_err).contains("no Xbox profile"));
    }

    #[test]
    fn oauth_errors_are_readable() {
        let e = oauth_error(OAuthError {
            error: "other".into(),
            error_description: "AADSTS1: Bad thing. Trace ID: 1 Correlation ID: 2".into(),
        });
        assert_eq!(e.to_string(), "Microsoft sign-in failed: AADSTS1: Bad thing.");
        let e = oauth_error(OAuthError { error: "invalid_grant".into(), error_description: String::new() });
        assert!(e.to_string().contains("expired"));
    }

    #[test]
    fn configured_client_id_wins() {
        assert_eq!(client_id("  abc ").unwrap(), "abc");
    }

    #[test]
    fn cancelling_stops_waiting() {
        let start = Instant::now();
        assert!(!sleep_unless_cancelled(Duration::from_secs(10), &|| true));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}

//! The account flow one HTTP step at a time, against [`AuthEndpoints`], with
//! the HTTP status and the service's error fields kept.
//!
//! [`crate::auth`] falls back to the interactive device-code flow on any
//! refresh error and does not look at status codes, so a host that must never
//! block on a human and must tell "the token was rejected" from "the service
//! is down" uses these instead. Nothing here is interactive: the device-code
//! functions are only for an operator command.

use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use reqwest::{RequestBuilder, Response, StatusCode, header::RETRY_AFTER};
use rsa::{RsaPrivateKey, pkcs8::DecodePrivateKey};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    AccessTokenResponse, DeviceCodeResponse, MinecraftAuthResponse, ProfileResponse,
    XboxLiveAuthResponse, certs::Certificates, certs::CertificatesResponse,
    endpoints::AuthEndpoints,
};

/// Nintendo Switch client id (works for accounts under 18), as [`crate::auth`].
pub const DEFAULT_CLIENT_ID: &str = "00000000441cc96b";
pub const DEFAULT_SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";

/// Which step of the flow failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    DeviceCode,
    DeviceToken,
    MsRefresh,
    XboxUser,
    Xsts,
    MinecraftLogin,
    Profile,
    Certificates,
    SessionJoin,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum StepError {
    /// The service answered with a non-success status.
    #[error("{step:?}: HTTP {status}{}{}: {body}",
        oauth_error.as_ref().map(|e| format!(" ({e})")).unwrap_or_default(),
        xerr.map(|e| format!(" XErr {e}")).unwrap_or_default())]
    Status {
        step: Step,
        status: u16,
        /// OAuth `error` field (`invalid_grant`, ...), or the Mojang `error`.
        oauth_error: Option<String>,
        /// Xbox `XErr` (no Xbox account, child account, ...).
        xerr: Option<u64>,
        /// `Retry-After` in seconds, when the service sent one.
        retry_after: Option<Duration>,
        /// The body, cut to 512 bytes.
        body: String,
    },
    /// Connection error or timeout: nothing was answered.
    #[error("{step:?}: transport error{}: {detail}", if *timeout { " (timeout)" } else { "" })]
    Transport {
        step: Step,
        detail: String,
        timeout: bool,
    },
    /// A success status with a body we can't read.
    #[error("{step:?}: bad response: {detail}")]
    Decode { step: Step, detail: String },
}

impl StepError {
    pub fn step(&self) -> Step {
        match self {
            StepError::Status { step, .. }
            | StepError::Transport { step, .. }
            | StepError::Decode { step, .. } => *step,
        }
    }
    pub fn status(&self) -> Option<u16> {
        match self {
            StepError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

fn transport(step: Step, e: reqwest::Error) -> StepError {
    StepError::Transport {
        step,
        timeout: e.is_timeout(),
        detail: e.to_string(),
    }
}

/// `Retry-After` as delta seconds (an HTTP date is not parsed: none of these
/// services send one).
fn retry_after(res: &Response) -> Option<Duration> {
    let v = res.headers().get(RETRY_AFTER)?.to_str().ok()?;
    v.trim().parse::<u64>().ok().map(Duration::from_secs)
}

async fn send(step: Step, req: RequestBuilder, timeout: Duration) -> Result<Response, StepError> {
    let res = req
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| transport(step, e))?;
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status().as_u16();
    let retry_after = retry_after(&res);
    let text = res.text().await.unwrap_or_default();
    let v: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    let oauth_error = v
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(|e| e.as_str())
        .map(str::to_owned);
    let xerr = v.as_ref().and_then(|v| v.get("XErr")).and_then(|e| e.as_u64());
    let mut body = text;
    if body.len() > 512 {
        let mut cut = 512;
        while !body.is_char_boundary(cut) {
            cut -= 1;
        }
        body.truncate(cut);
    }
    Err(StepError::Status {
        step,
        status,
        oauth_error,
        xerr,
        retry_after,
        body,
    })
}

async fn json<T: DeserializeOwned>(step: Step, res: Response) -> Result<T, StepError> {
    let text = res.text().await.map_err(|e| transport(step, e))?;
    serde_json::from_str(&text).map_err(|e| StepError::Decode {
        step,
        detail: e.to_string(),
    })
}

/// Per-request timeout used by the steps below.
#[derive(Clone, Copy, Debug)]
pub struct StepTimeout(pub Duration);
impl Default for StepTimeout {
    fn default() -> Self {
        Self(Duration::from_secs(20))
    }
}

/// Refresh-token grant. Microsoft may rotate the refresh token: the caller
/// must store `refresh_token` of the answer before it uses the access token.
pub async fn refresh_ms(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    refresh_token: &str,
    client_id: Option<&str>,
    scope: Option<&str>,
    t: StepTimeout,
) -> Result<AccessTokenResponse, StepError> {
    let client_id = client_id.unwrap_or(DEFAULT_CLIENT_ID);
    let scope = scope.unwrap_or(DEFAULT_SCOPE);
    let req = client.post(&ep.ms_token).form(&[
        ("scope", scope),
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ]);
    let res = send(Step::MsRefresh, req, t.0).await?;
    json(Step::MsRefresh, res).await
}

/// Xbox Live user token and user hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XboxUser {
    pub token: String,
    pub user_hash: String,
    pub not_after: Option<DateTime<Utc>>,
}

pub async fn xbox_user(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    ms_access_token: &str,
    t: StepTimeout,
) -> Result<XboxUser, StepError> {
    let body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": ms_access_token
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });
    let req = client
        .post(&ep.xbox_user)
        .header("Accept", "application/json")
        .header("x-xbl-contract-version", "1")
        .json(&body);
    let res = send(Step::XboxUser, req, t.0).await?;
    let r: XboxLiveAuthResponse = json(Step::XboxUser, res).await?;
    let user_hash = r
        .display_claims
        .get("xui")
        .and_then(|x| x.first())
        .and_then(|x| x.get("uhs"))
        .cloned()
        .ok_or_else(|| StepError::Decode {
            step: Step::XboxUser,
            detail: "no xui[0].uhs claim".into(),
        })?;
    Ok(XboxUser {
        token: r.token,
        user_hash,
        not_after: DateTime::parse_from_rfc3339(&r.not_after)
            .ok()
            .map(|d| d.with_timezone(&Utc)),
    })
}

/// XSTS token for Minecraft.
pub async fn xsts(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    xbl_token: &str,
    t: StepTimeout,
) -> Result<String, StepError> {
    let req = client
        .post(&ep.xsts)
        .header("Accept", "application/json")
        .json(&json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbl_token]
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }));
    let res = send(Step::Xsts, req, t.0).await?;
    let r: XboxLiveAuthResponse = json(Step::Xsts, res).await?;
    Ok(r.token)
}

/// Minecraft access token (`expires_in` seconds, usually 24 h).
pub async fn minecraft_login(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    user_hash: &str,
    xsts_token: &str,
    t: StepTimeout,
) -> Result<MinecraftAuthResponse, StepError> {
    let req = client
        .post(&ep.mc_login)
        .header("Accept", "application/json")
        .json(&json!({ "identityToken": format!("XBL3.0 x={user_hash};{xsts_token}") }));
    let res = send(Step::MinecraftLogin, req, t.0).await?;
    json(Step::MinecraftLogin, res).await
}

/// The Minecraft profile; 404 means the account has no Minecraft profile.
pub async fn profile(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    mc_access_token: &str,
    t: StepTimeout,
) -> Result<ProfileResponse, StepError> {
    let req = client
        .get(&ep.mc_profile)
        .header("Authorization", format!("Bearer {mc_access_token}"));
    let res = send(Step::Profile, req, t.0).await?;
    json(Step::Profile, res).await
}

/// Player certificates (profile key pair, about 48 h validity).
pub async fn certificates(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    mc_access_token: &str,
    t: StepTimeout,
) -> Result<Certificates, StepError> {
    let req = client
        .post(&ep.mc_certificates)
        .header("Authorization", format!("Bearer {mc_access_token}"));
    let res = send(Step::Certificates, req, t.0).await?;
    let r: CertificatesResponse = json(Step::Certificates, res).await?;
    decode_certificates(r).map_err(|detail| StepError::Decode {
        step: Step::Certificates,
        detail,
    })
}

fn pem_body(pem: &str) -> Result<Vec<u8>, String> {
    let b64: String = pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .map(str::trim)
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("pem base64: {e}"))
}

/// Certificates from the service's answer, without panicking on bad input.
pub fn decode_certificates(r: CertificatesResponse) -> Result<Certificates, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let private_key_der = pem_body(&r.key_pair.private_key)?;
    let private_key =
        RsaPrivateKey::from_pkcs8_der(&private_key_der).map_err(|e| format!("pkcs8: {e}"))?;
    Ok(Certificates {
        private_key,
        public_key_der: pem_body(&r.key_pair.public_key)?,
        signature_v1: b64
            .decode(&r.public_key_signature)
            .map_err(|e| format!("signature v1: {e}"))?,
        signature_v2: b64
            .decode(&r.public_key_signature_v2)
            .map_err(|e| format!("signature v2: {e}"))?,
        expires_at: r.expires_at,
        refresh_after: r.refreshed_after,
    })
}

#[derive(Deserialize)]
struct Forbidden {
    error: Option<String>,
}

/// Sessionserver join at `ep.session_join`. 204 is success; anything else is
/// a [`StepError::Status`] whose `oauth_error` holds Mojang's `error` (for
/// example `ForbiddenOperationException`: the access token is not valid).
pub async fn session_join(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    access_token: &str,
    uuid: &Uuid,
    server_hash: &str,
    t: StepTimeout,
) -> Result<(), StepError> {
    let mut buf = Uuid::encode_buffer();
    let undashed = uuid.simple().encode_lower(&mut buf);
    let req = client.post(&ep.session_join).json(&json!({
        "accessToken": access_token,
        "selectedProfile": undashed,
        "serverId": server_hash
    }));
    match send(Step::SessionJoin, req, t.0).await {
        Ok(res) if res.status() == StatusCode::NO_CONTENT || res.status() == StatusCode::OK => {
            Ok(())
        }
        Ok(res) => Err(StepError::Status {
            step: Step::SessionJoin,
            status: res.status().as_u16(),
            oauth_error: None,
            xerr: None,
            retry_after: None,
            body: String::new(),
        }),
        Err(StepError::Status {
            step,
            status,
            oauth_error,
            xerr,
            retry_after,
            body,
        }) => {
            let mojang = serde_json::from_str::<Forbidden>(&body)
                .ok()
                .and_then(|f| f.error);
            Err(StepError::Status {
                step,
                status,
                oauth_error: mojang.or(oauth_error),
                xerr,
                retry_after,
                body,
            })
        }
        Err(e) => Err(e),
    }
}

/// Device-code request. Interactive: for an operator command only.
pub async fn device_code(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    client_id: Option<&str>,
    scope: Option<&str>,
    t: StepTimeout,
) -> Result<DeviceCodeResponse, StepError> {
    let req = client.post(&ep.ms_device_code).form(&[
        ("scope", scope.unwrap_or(DEFAULT_SCOPE)),
        ("client_id", client_id.unwrap_or(DEFAULT_CLIENT_ID)),
        ("response_type", "device_code"),
    ]);
    let res = send(Step::DeviceCode, req, t.0).await?;
    json(Step::DeviceCode, res).await
}

/// One poll of the device-code grant: `Ok(None)` while the user has not
/// finished (`authorization_pending`, `slow_down`).
pub async fn device_token_poll(
    client: &reqwest::Client,
    ep: &AuthEndpoints,
    device_code: &str,
    client_id: Option<&str>,
    t: StepTimeout,
) -> Result<Option<AccessTokenResponse>, StepError> {
    let client_id = client_id.unwrap_or(DEFAULT_CLIENT_ID);
    let req = client.post(&ep.ms_token).form(&[
        ("client_id", client_id),
        ("device_code", device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ]);
    match send(Step::DeviceToken, req, t.0).await {
        Ok(res) => json(Step::DeviceToken, res).await.map(Some),
        Err(StepError::Status {
            oauth_error: Some(e),
            ..
        }) if e == "authorization_pending" || e == "slow_down" => Ok(None),
        Err(e) => Err(e),
    }
}

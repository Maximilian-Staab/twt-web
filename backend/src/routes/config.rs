//! Unauthenticated OAuth bootstrap helpers.
//!
//! GET /config — issuer, public client id, scopes and flow for all clients.
//! GET /oidc-config — server-side discovery proxy for browser clients.
//!
//! Browser clients need two server-side relays because IdPs are not obliged
//! to send CORS headers on their discovery document or token endpoint
//! (authentik, for example, does not send them on either):
//!
//! * `/oidc-config` fetches `<issuer>/.well-known/openid-configuration`.
//! * `/oauth/token` forwards an authorization-code or refresh-token grant to
//!   the token endpoint. Only the configured public client is accepted, and
//!   the upstream URL comes from server-side discovery — never from the
//!   request — so this is not an open SSRF proxy.

use std::time::Duration;

use axum::http::{header, StatusCode};
use axum::Form;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::Problem;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
pub struct TokenForm {
    pub grant_type: String,
    pub code: Option<String>,
    pub code_verifier: Option<String>,
    pub redirect_uri: Option<String>,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

pub async fn get_oidc_config() -> Result<Json<Value>, Problem> {
    Ok(Json(fetch_discovery().await?))
}

/// Exchanges a code/refresh token with the configured IdP. The response body
/// and status are returned unchanged so the browser can apply the same OAuth
/// error handling as with a direct token request.
pub async fn post_token(Form(form): Form<TokenForm>) -> Result<(StatusCode, Json<Value>), Problem> {
    let client_id = env_or("OAUTH_CLIENT_ID", "");
    if client_id.is_empty() {
        return Err(Problem::validation("OAUTH_CLIENT_ID is not configured"));
    }
    if let Some(supplied) = form.client_id.as_deref() {
        if supplied != client_id {
            return Err(Problem::validation(
                "client_id does not match the configured client",
            ));
        }
    }

    let discovery = fetch_discovery().await?;
    let token_endpoint = discovery
        .get("token_endpoint")
        .and_then(Value::as_str)
        .ok_or_else(|| upstream("discovery document has no token_endpoint"))?;

    let mut fields: Vec<(&str, String)> = vec![
        ("grant_type", form.grant_type.clone()),
        ("client_id", client_id),
    ];
    match form.grant_type.as_str() {
        "authorization_code" => {
            fields.push(("code", require(form.code, "code")?));
            fields.push((
                "code_verifier",
                require(form.code_verifier, "code_verifier")?,
            ));
            fields.push(("redirect_uri", require(form.redirect_uri, "redirect_uri")?));
        }
        "refresh_token" => {
            fields.push((
                "refresh_token",
                require(form.refresh_token, "refresh_token")?,
            ));
        }
        other => {
            return Err(Problem::validation(format!(
                "unsupported grant_type: {other}"
            )))
        }
    }

    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| upstream(format!("build HTTP client: {e}")))?;
    let response = client
        .post(token_endpoint)
        .header(header::ACCEPT, "application/json")
        .form(&fields)
        .send()
        .await
        .map_err(|e| upstream(format!("fetch {token_endpoint}: {e}")))?;
    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let document: Value = response
        .json()
        .await
        .map_err(|e| upstream(format!("parse {token_endpoint}: {e}")))?;
    Ok((status, Json(document)))
}

pub async fn get_config() -> Json<Value> {
    let scopes: Vec<String> = env_or("OAUTH_SCOPES", "openid,profile,offline_access")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    Json(json!({
        "issuer": env_or("OAUTH_ISSUER_URL", ""),
        "clientId": env_or("OAUTH_CLIENT_ID", ""),
        "scopes": scopes,
        "flow": "authorization_code_pkce",
        // Browser clients exchange the code through POST /oauth/token.
        "tokenProxy": true,
    }))
}

fn require(value: Option<String>, name: &str) -> Result<String, Problem> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(Problem::validation(format!("missing {name}"))),
    }
}

async fn fetch_discovery() -> Result<Value, Problem> {
    let issuer = env_or("OAUTH_ISSUER_URL", "");
    if issuer.trim().is_empty() {
        return Err(upstream("OAUTH_ISSUER_URL is not configured"));
    }
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/'),
    );
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| upstream(format!("build HTTP client: {e}")))?;
    let response = client
        .get(&url)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|e| upstream(format!("fetch {url}: {e}")))?;
    if !response.status().is_success() {
        return Err(upstream(format!("fetch {url}: HTTP {}", response.status())));
    }
    let document: Value = response
        .json()
        .await
        .map_err(|e| upstream(format!("parse {url}: {e}")))?;
    Ok(document)
}

fn upstream(detail: impl Into<String>) -> Problem {
    Problem::new(StatusCode::BAD_GATEWAY, "upstream", "Upstream error").with_detail(detail.into())
}

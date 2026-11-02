#![allow(dead_code)]
//! Requires a Postgres reachable at `TEST_DATABASE_URL` (or the standard
//! `PGHOST`/`PGPORT` env vars). Resolution order:
//! 1. `TEST_DATABASE_URL` — a URL to the admin `postgres` DB
//! 2. fallback: `postgres://twt:twt-dev@127.0.0.1:5432/postgres`
//!    (the devenv `backend` profile's Postgres; `devenv up` starts it)
//!
//! Tests forge RS256 JWTs signed by `dev/dev-key.pem`, matching
//! `dev/jwks.json`, so the server's static-JWKS auth path is exercised end
//! to end.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::Value;
use tower::ServiceExt;

use twt_cloud::api::AppState;
use twt_cloud::auth::{AuthConfig, VerifiedKey};

pub struct TestServer {
    pub app: Router,
    pub db_url: String,
}

/// Builds a fresh scratch DB + migrated pool, assembles the router on it.
pub async fn spawn_server() -> TestServer {
    let base = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://twt:twt-dev@127.0.0.1:5432/postgres".to_string());
    let db_name = format!("twt_test_{}", uuid::Uuid::new_v4().simple());
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&base)
        .await
        .expect("connect TEST_DATABASE_URL");

    // The name is `twt_test_<uuid>`, so the identifier is safe to interpolate.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"CREATE DATABASE "{}""#,
        db_name
    )))
    .execute(&admin)
    .await
    .expect("create scratch db");
    admin.close().await;

    let db_url = rewrite_db(&base, &db_name);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("connect scratch db");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");

    let state = AppState {
        pool: pool.clone(),
        auth: test_auth_config(),
    };
    let app = twt_cloud::api::router(state);
    TestServer { app, db_url }
}

/// Drops the scratch DB (terminate backends first; DROP fails otherwise).
pub async fn drop_db(db_url: &str) {
    let (admin_url, db_name) = admin_parts(db_url);
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect admin");
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = $1 AND pid <> pg_backend_pid()",
    )
    .bind(&db_name)
    .execute(&admin)
    .await
    .ok();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"DROP DATABASE IF EXISTS "{}""#,
        db_name
    )))
    .execute(&admin)
    .await
    .expect("drop scratch db");
    admin.close().await;
}

/// Splits `postgres://user:pass@host:port/dbname` into (admin url, dbname)
/// using plain string ops — no url crate needed.
fn admin_parts(db_url: &str) -> (String, String) {
    let (prefix, rest) = db_url.split_once("://").expect("db url scheme");
    let (authority, path) = rest.split_once('/').expect("db url host/path");
    let db_name = path.split('/').next().unwrap_or_default().to_string();
    (format!("{prefix}://{authority}/postgres"), db_name)
}

fn rewrite_db(base: &str, db_name: &str) -> String {
    let (prefix, rest) = base.split_once("://").expect("db url scheme");
    let (authority, _path) = rest.split_once('/').expect("db url host/path");
    format!("{prefix}://{authority}/{db_name}")
}

/// AuthConfig matching the live dev setup: static JWKS key, no refresh.
pub fn test_auth_config() -> AuthConfig {
    AuthConfig {
        issuer: "https://dev-idp.example.com".into(),
        audience: "twt-api".into(),
        keys: std::sync::Arc::new(parking_lot::RwLock::new(vec![test_key()])),
        jwks_url: None,
        last_refresh: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        refresh_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        client: reqwest::Client::new(),
        clock_skew: 30,
    }
}

pub fn test_key() -> VerifiedKey {
    let jwks: jsonwebtoken::jwk::JwkSet =
        serde_json::from_reader(std::fs::File::open(jwks_path()).expect("read jwks.json"))
            .expect("parse jwks");
    let jwk = jwks.keys.into_iter().next().expect("jwks has a key");
    let key = jsonwebtoken::DecodingKey::from_jwk(&jwk).expect("decoding key from jwk");
    VerifiedKey {
        kid: "dev-key-1".into(),
        key,
    }
}

fn jwks_path() -> PathBuf {
    dev_key_path().parent().unwrap().join("jwks.json")
}

pub fn dev_key_path() -> PathBuf {
    // tests run with CWD = backend crate root (cargo test)
    let mut p = std::env::current_dir().expect("cwd");
    while !p.join("dev/dev-key.pem").exists() && p.parent().is_some() {
        p = p.parent().unwrap().to_path_buf();
    }
    p.join("dev/dev-key.pem")
}

/// Claims for a forged token.
#[derive(serde::Serialize)]
pub struct TestClaims {
    pub sub: String,
    pub iss: String,
    pub aud: String,
    pub exp: u64,
    pub scope: Option<String>,
}

/// Signs an RS256 JWT with the dev key; validated by the server against
/// `dev/jwks.json` (kid `dev-key-1`).
pub fn forge_token(sub: &str) -> String {
    let key_pem = std::fs::read_to_string(dev_key_path()).expect("read dev-key.pem");
    let enc_key = EncodingKey::from_rsa_pem(key_pem.as_bytes()).expect("encoding key");
    let header = Header {
        alg: jsonwebtoken::Algorithm::RS256,
        kid: Some("dev-key-1".into()),
        ..Default::default()
    };
    let claims = TestClaims {
        sub: sub.into(),
        iss: "https://dev-idp.example.com".into(),
        aud: "twt-api".into(),
        exp: chrono::Utc::now().timestamp() as u64 + 3600,
        scope: Some("twt.data".into()),
    };
    encode(&header, &claims, &enc_key).expect("sign token")
}

// ---------------------------------------------------------------------------
// request helpers (tower::oneshot against the in-process router)
// ---------------------------------------------------------------------------

pub async fn get(app: &Router, token: &str, path: &str) -> (StatusCode, Value) {
    let req = Request::get(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("request");
    (res.status(), body_json(res).await)
}

pub async fn post(app: &Router, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
    req_with_body(app, token, axum::http::Method::POST, path, body).await
}

pub async fn put(app: &Router, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
    req_with_body(app, token, axum::http::Method::PUT, path, body).await
}

pub async fn patch(app: &Router, token: &str, path: &str, body: Value) -> (StatusCode, Value) {
    req_with_body(app, token, axum::http::Method::PATCH, path, body).await
}

pub async fn delete(app: &Router, token: &str, path: &str) -> (StatusCode, Value) {
    let req = Request::delete(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("request");
    (res.status(), body_json(res).await)
}

async fn req_with_body(
    app: &Router,
    token: &str,
    method: axum::http::Method,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("request");
    (res.status(), body_json(res).await)
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes).expect("parse json body")
}

/// Raw-body GET (CSV export returns text, not JSON).
pub async fn get_raw(app: &Router, token: &str, path: &str) -> (StatusCode, String) {
    let req = Request::get(path)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("request");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Raw-body PUT (CSV import sends text/csv).
pub async fn put_raw_text(
    app: &Router,
    token: &str,
    path: &str,
    body: String,
) -> (StatusCode, String) {
    let req = Request::put(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "text/csv")
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.expect("request");
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

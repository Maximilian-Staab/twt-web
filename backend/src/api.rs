//! Router assembly and application state.

use std::time::Duration;

use axum::http::{header, HeaderValue, Method};
use axum::routing::{get, post, put};
use axum::Router;
use sqlx::PgPool;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::auth::AuthConfig;
use crate::routes::{
    backup, clients, config, events, identity, preferences, snapshot, sync, targets, tasks,
};
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub auth: AuthConfig,
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/healthz", get(identity::health))
        .route("/config", get(config::get_config))
        .route("/oidc-config", get(config::get_oidc_config))
        .route("/oauth/token", post(config::post_token))
        .route("/me", get(identity::get_me))
        .route(
            "/clients",
            get(clients::list_clients).post(clients::create_client),
        )
        .route(
            "/clients/{client_id}",
            axum::routing::delete(clients::revoke_client),
        )
        .route("/tasks", get(tasks::list_tasks).post(tasks::create_task))
        .route(
            "/tasks/{id}",
            get(tasks::get_task)
                .patch(tasks::update_task)
                .delete(tasks::delete_task),
        )
        .route(
            "/events",
            get(events::list_events).post(events::create_event),
        )
        .route(
            "/events/{id}",
            get(events::get_event)
                .patch(events::update_event)
                .delete(events::delete_event),
        )
        .route(
            "/targets",
            get(targets::list_targets).post(targets::create_target),
        )
        .route(
            "/targets/{id}",
            get(targets::get_target)
                .patch(targets::update_target)
                .delete(targets::delete_target),
        )
        .route("/preferences", get(preferences::list_preferences))
        .route("/preferences/{key}", put(preferences::put_preference))
        .route("/sync/changes", get(sync::get_changes))
        .route(
            "/sync/push",
            axum::routing::post(sync::push_batch)
                .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/sync/state", get(sync::sync_state))
        .route(
            "/backup/events",
            get(backup::backup_events_get).put(backup::backup_events_put),
        )
        .route(
            "/backup/targets",
            get(backup::backup_targets_get).put(backup::backup_targets_put),
        )
        .route(
            "/backup/preferences",
            get(backup::backup_prefs_get).put(backup::backup_prefs_put),
        )
        .route("/export", get(snapshot::export_all))
        .route("/import", put(snapshot::import_all))
        .with_state(state);

    let app = Router::new().nest("/api/v1", api);
    match cors_layer() {
        Some(cors) => app.layer(cors),
        None => app,
    }
}

/// Browser clients (the web frontend) call the API cross-origin. CORS is
/// opt-in: without `CORS_ALLOWED_ORIGINS` the API emits no CORS headers and
/// stays usable only from same-origin deployments (reverse proxy).
///
/// The variable is a comma-separated list of exact origins
/// (`https://twt.example.com`), or `*` to allow any origin. Bearer auth is
/// unaffected by CORS's credential rules; `*` is only appropriate for
/// deployments that do not mind any site calling the API with a user-supplied
/// token.
fn cors_layer() -> Option<CorsLayer> {
    let raw = std::env::var("CORS_ALLOWED_ORIGINS").ok()?;
    let mut origins = Vec::new();
    let mut any = false;
    for origin in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if origin == "*" {
            any = true;
            continue;
        }
        // A trailing slash or a path never matches the browser's `Origin`
        // header (which is `scheme://host[:port]`). Normalize the common
        // copy-pasted form and reject the rest with a visible warning.
        // Browsers serialize `Origin` with a lowercase scheme and host.
        let origin = origin.trim_end_matches('/').to_ascii_lowercase();
        let Some((scheme, authority)) = origin.split_once("://") else {
            tracing::warn!(origin, "ignoring CORS origin without a scheme");
            continue;
        };
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            tracing::warn!(origin, "ignoring CORS origin with an unsupported scheme");
            continue;
        }
        if authority.is_empty() || authority.contains('/') {
            tracing::warn!(origin, "ignoring CORS origin with a path");
            continue;
        }
        match HeaderValue::from_str(&origin) {
            Ok(value) => origins.push(value),
            Err(_) => tracing::warn!(origin, "ignoring invalid CORS origin"),
        }
    }
    if !any && origins.is_empty() {
        return None;
    }
    let allow_origin = if any {
        AllowOrigin::any()
    } else {
        AllowOrigin::list(origins)
    };
    Some(
        CorsLayer::new()
            .allow_origin(allow_origin)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT])
            .max_age(Duration::from_secs(86_400)),
    )
}

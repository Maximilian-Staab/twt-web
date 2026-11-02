//! GET /healthz, GET /me

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::ensure_user;

pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

pub async fn get_me(
    State(state): State<AppState>,
    auth: crate::routes::Auth,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let row: Option<(chrono::DateTime<chrono::Utc>,)> =
        sqlx::query_as("SELECT created_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await?;
    let created_at = row.map(|r| r.0).unwrap_or_else(chrono::Utc::now);
    Ok(Json(json!({ "sub": auth.0.sub, "createdAt": created_at })))
}

//! /preferences — typed key/value store (OpenAPI tag `preferences`).
//!
//! Mirrors `PreferencesUtil.writePreferences`: types `string`, `stringset`,
//! `boolean`, `integer`, `float`, `long`. Stored as JSONB; LWW per key.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{Map, Value};

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::{append_change_tx, ensure_user, Auth};

pub async fn list_preferences(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT key, value FROM preferences WHERE user_id = $1 ORDER BY key")
            .bind(user_id)
            .fetch_all(&state.pool)
            .await?;
    let mut map = Map::new();
    for (key, value) in rows {
        map.insert(key, value);
    }
    Ok(Json(Value::Object(map)))
}

pub async fn put_preference(
    State(state): State<AppState>,
    auth: Auth,
    Path(key): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let now = chrono::Utc::now();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO preferences (user_id, key, value, updated_at) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at",
    )
    .bind(user_id)
    .bind(&key)
    .bind(&body)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    append_change_tx(
        &mut tx,
        user_id,
        "preference",
        &key,
        "upsert",
        body.clone(),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::OK, Json(body)))
}

//! /clients — device registrations (OpenAPI tag `clients`).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::{append_change_tx, ensure_user, Auth};

#[derive(Deserialize)]
pub struct ClientCreate {
    pub label: String,
}

pub async fn list_clients(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let rows: Vec<(
        Uuid,
        String,
        DateTime<Utc>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT client_id, label, created_at, last_seen_at, revoked_at \
         FROM clients WHERE user_id = $1 AND revoked_at IS NULL ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    let clients: Vec<Value> = rows
        .into_iter()
        .map(|(id, label, created, last_seen, revoked)| {
            json!({
                "clientId": id,
                "label": label,
                "createdAt": created,
                "lastSeenAt": last_seen,
                "revokedAt": revoked,
            })
        })
        .collect();
    Ok(Json(Value::Array(clients)))
}

pub async fn create_client(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<ClientCreate>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let client_id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query("INSERT INTO clients (client_id, user_id, label) VALUES ($1, $2, $3)")
        .bind(client_id)
        .bind(user_id)
        .bind(&body.label)
        .execute(&state.pool)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "clientId": client_id,
            "label": body.label,
            "createdAt": now,
            "lastSeenAt": now,
            "revokedAt": null,
        })),
    ))
}

pub async fn revoke_client(
    State(state): State<AppState>,
    auth: Auth,
    Path(client_id): Path<Uuid>,
) -> Result<StatusCode, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let mut tx = state.pool.begin().await?;
    let res = sqlx::query(
        "UPDATE clients SET revoked_at = now() \
         WHERE client_id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(client_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(Problem::not_found());
    }
    append_change_tx(
        &mut tx,
        user_id,
        "client",
        &client_id.to_string(),
        "delete",
        serde_json::Value::Null,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

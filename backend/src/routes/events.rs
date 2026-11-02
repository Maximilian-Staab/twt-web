//! /events — event CRUD (OpenAPI tag `events`).
//!
//! Units (grounded in `DAO.eventToContentValues` / `cursorToEvent`):
//! * `time` — epoch SECONDS (stored as-is; the app's DAO truncates to whole
//!   minutes only when reading, so we keep full second precision)
//! * `zone_offset` — UTC offset in MINUTES
//! * `type` — `CLOCK_IN`/`CLOCK_OUT` only; `CLOCK_OUT_NOW` (-1) is
//!   client-synthesized and never stored, so it is rejected here.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::{alloc_seq, append_change_tx, ensure_user, Auth};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventUpsert {
    pub time: i64,
    pub zone_offset: i32,
    pub r#type: String,
    pub task: Option<Uuid>,
    pub comment: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub include_deleted: Option<bool>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// `updatedAt` query param: optional LWW precondition on deletes.
#[derive(Deserialize)]
pub struct UpdatedAtParam {
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_events(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<ListParams>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let include = q.include_deleted.unwrap_or(false);
    // Order (time, insertion_seq) reproduces the app's (time, _id) order:
    // the engine is order-sensitive for same-time events (a stray OUT is
    // ignored, a stray IN keeps the session open), so ties must break the
    // way the app breaks them. `id` is a final deterministic tiebreak.
    let rows: Vec<(
        Uuid,
        i64,
        i32,
        String,
        Option<Uuid>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "SELECT id, time, zone_offset, type::text, task, comment, updated_at, deleted_at \
         FROM events WHERE user_id = $1 AND ($2 OR deleted_at IS NULL) \
         AND ($3::bigint IS NULL OR time >= $3) \
         AND ($4::bigint IS NULL OR time < $4) \
         ORDER BY time, insertion_seq, id LIMIT $5 OFFSET $6",
    )
    .bind(user_id)
    .bind(include)
    .bind(q.from)
    .bind(q.to)
    .bind(q.limit.unwrap_or(500).clamp(1, 1000))
    .bind(q.offset.unwrap_or(0).max(0))
    .fetch_all(&state.pool)
    .await?;
    let events: Vec<Value> = rows
        .into_iter()
        .map(
            |(id, time, zone_offset, r#type, task, comment, updated_at, deleted_at)| {
                json!({
                    "id": id,
                    "time": time,
                    "zoneOffset": zone_offset,
                    "type": r#type,
                    "task": task,
                    "comment": comment,
                    "updatedAt": updated_at,
                    "deletedAt": deleted_at,
                })
            },
        )
        .collect();
    Ok(Json(Value::Array(events)))
}

pub async fn create_event(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<EventUpsert>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    if body.r#type != "CLOCK_IN" && body.r#type != "CLOCK_OUT" {
        return Err(Problem::validation(format!(
            "invalid event type: {}",
            body.r#type
        )));
    }
    if body.time < 0 {
        return Err(Problem::validation("time must be >= 0 (epoch seconds)"));
    }
    if !crate::routes::zone_offset_ok(body.zone_offset as i64) {
        return Err(Problem::validation(
            "zoneOffset must be within ±1080 minutes (±18h)",
        ));
    }
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let mut tx = state.pool.begin().await?;
    let insertion_seq = alloc_seq(&mut *tx, user_id).await?;
    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO events (id, user_id, time, zone_offset, type, task, comment, updated_at, insertion_seq) \
         VALUES ($1, $2, $3, $4, $5::event_type, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(user_id)
    .bind(body.time)
    .bind(body.zone_offset)
    .bind(body.r#type.clone())
    .bind(body.task)
    .bind(body.comment.clone())
    .bind(now)
    .bind(insertion_seq)
    .execute(&mut *tx)
    .await?;
    append_change_tx(
        &mut tx,
        user_id,
        "event",
        &id.to_string(),
        "upsert",
        json!({
            "id": id, "time": body.time, "zoneOffset": body.zone_offset, "type": body.r#type,
            "task": body.task, "comment": body.comment, "updatedAt": now, "deletedAt": null,
        }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "time": body.time,
            "zoneOffset": body.zone_offset,
            "type": body.r#type,
            "task": body.task,
            "comment": body.comment,
            "updatedAt": now,
            "deletedAt": null,
        })),
    ))
}

pub async fn get_event(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let row: Option<(
        Uuid,
        i64,
        i32,
        String,
        Option<Uuid>,
        Option<String>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT id, time, zone_offset, type::text, task, comment, updated_at, deleted_at \
         FROM events WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, time, zone_offset, r#type, task, comment, updated_at, deleted_at)) = row else {
        return Err(Problem::not_found());
    };
    Ok(Json(json!({
        "id": id,
        "time": time,
        "zoneOffset": zone_offset,
        "type": r#type,
        "task": task,
        "comment": comment,
        "updatedAt": updated_at,
        "deletedAt": deleted_at,
    })))
}

pub async fn update_event(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<EventUpsert>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    if body.r#type != "CLOCK_IN" && body.r#type != "CLOCK_OUT" {
        return Err(Problem::validation(format!(
            "invalid event type: {}",
            body.r#type
        )));
    }
    if body.time < 0 {
        return Err(Problem::validation("time must be >= 0 (epoch seconds)"));
    }
    if !crate::routes::zone_offset_ok(body.zone_offset as i64) {
        return Err(Problem::validation(
            "zoneOffset must be within ±1080 minutes (±18h)",
        ));
    }
    let mut tx = state.pool.begin().await?;
    // RETURNING captures the full post-update row inside the tx: the same
    // row is both the PATCH response body and the change-log payload, so
    // pulls can apply REST updates (CloudSyncManager.applyEventChange
    // requires time+type; a bare {"id"} payload is dropped as sparse).
    let row: Option<(
        i64,
        i32,
        String,
        Option<Uuid>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "UPDATE events SET \
            time = COALESCE($3, time), \
            zone_offset = COALESCE($4, zone_offset), \
            type = COALESCE($5::event_type, type), \
            task = COALESCE($6, task), \
            comment = COALESCE($7, comment), \
            updated_at = now() \
         WHERE id = $1 AND user_id = $2 \
           AND ($8::timestamptz IS NULL OR updated_at <= $8::timestamptz) \
         RETURNING time, zone_offset, type::text, task, comment, updated_at, deleted_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(body.time)
    .bind(body.zone_offset)
    .bind(body.r#type.clone())
    .bind(body.task)
    .bind(body.comment.clone())
    .bind(body.updated_at)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((time, zone_offset, r#type, task, comment, updated_at, deleted_at)) = row else {
        // No row: either the LWW precondition filtered it out, or it's absent.
        let exists: Option<(bool,)> = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM events WHERE id = $1 AND user_id = $2) AS e",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if exists.map(|(e,)| e).unwrap_or(false) {
            return Err(Problem::precondition_failed());
        }
        return Err(Problem::not_found());
    };
    append_change_tx(
        &mut tx,
        user_id,
        "event",
        &id.to_string(),
        "upsert",
        json!({
            "id": id, "time": time, "zoneOffset": zone_offset, "type": r#type,
            "task": task, "comment": comment, "updatedAt": updated_at, "deletedAt": deleted_at,
        }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "id": id,
        "time": time,
        "zoneOffset": zone_offset,
        "type": r#type,
        "task": task,
        "comment": comment,
        "updatedAt": updated_at,
        "deletedAt": deleted_at,
    })))
}

pub async fn delete_event(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Query(pre): Query<UpdatedAtParam>,
) -> Result<StatusCode, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let pre = pre.updated_at;
    let mut tx = state.pool.begin().await?;
    let res = sqlx::query(
        "UPDATE events SET deleted_at = now(), updated_at = now() \
         WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL \
         AND ($3::timestamptz IS NULL OR updated_at <= $3::timestamptz)",
    )
    .bind(id)
    .bind(user_id)
    .bind(pre)
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        // Either not found / already deleted, or the LWW precondition failed.
        let exists: Option<(bool,)> = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM events WHERE id = $1 AND user_id = $2) AS e",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
        if exists.map(|(e,)| e).unwrap_or(false) {
            return Err(Problem::precondition_failed());
        }
        return Err(Problem::not_found());
    }
    append_change_tx(
        &mut tx,
        user_id,
        "event",
        &id.to_string(),
        "delete",
        Value::Null,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

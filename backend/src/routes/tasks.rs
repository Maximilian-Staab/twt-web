//! /tasks — task CRUD (OpenAPI tag `tasks`).
//!
//! Mirrors the app's `tasks` table (MySQLiteHelper v5): `ordering` nullable,
//! `active`/`isdefault` as booleans (app uses 0/1 INTEGERs).

use axum::extract::{Path, Query, State};
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
#[serde(rename_all = "camelCase")]
pub struct TaskUpsert {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub ordering: Option<i32>,
    pub is_default: Option<bool>,
    /// LWW precondition: when Some and older than the stored row's
    /// `updated_at`, the update fails with 412 (openapi TaskUpsert).
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub include_deleted: Option<bool>,
}

/// `updatedAt` query param: optional LWW precondition on deletes.
#[derive(Deserialize)]
pub struct UpdatedAtParam {
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_tasks(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<ListParams>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let include = q.include_deleted.unwrap_or(false);
    let rows: Vec<(
        Uuid,
        String,
        bool,
        Option<i32>,
        bool,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT id, name, active, ordering, is_default, updated_at, deleted_at \
         FROM tasks WHERE user_id = $1 AND ($2 OR deleted_at IS NULL) ORDER BY name",
    )
    .bind(user_id)
    .bind(include)
    .fetch_all(&state.pool)
    .await?;
    let tasks: Vec<Value> = rows
        .into_iter()
        .map(
            |(id, name, active, ordering, is_default, updated_at, deleted_at)| {
                json!({
                    "id": id,
                    "name": name,
                    "active": active,
                    "ordering": ordering,
                    "isDefault": is_default,
                    "updatedAt": updated_at,
                    "deletedAt": deleted_at,
                })
            },
        )
        .collect();
    Ok(Json(Value::Array(tasks)))
}

pub async fn create_task(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<TaskUpsert>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let name = body.name.clone().unwrap_or_default();
    let active = body.active.unwrap_or(false);
    let ordering = body.ordering;
    let is_default = body.is_default.unwrap_or(false);
    let id = Uuid::new_v4();
    let now = Utc::now();
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO tasks (id, user_id, name, active, ordering, is_default, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(user_id)
    .bind(&name)
    .bind(active)
    .bind(ordering)
    .bind(is_default)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    append_change_tx(
        &mut tx,
        user_id,
        "task",
        &id.to_string(),
        "upsert",
        json!({
            "id": id,
            "name": name,
            "active": active,
            "ordering": ordering,
            "isDefault": is_default,
            "updatedAt": now,
            "deletedAt": null,
        }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": name,
            "active": active,
            "ordering": ordering,
            "isDefault": is_default,
            "updatedAt": now,
            "deletedAt": null,
        })),
    ))
}

pub async fn get_task(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let row: Option<(
        Uuid,
        String,
        bool,
        Option<i32>,
        bool,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT id, name, active, ordering, is_default, updated_at, deleted_at \
         FROM tasks WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, name, active, ordering, is_default, updated_at, deleted_at)) = row else {
        return Err(Problem::not_found());
    };
    Ok(Json(json!({
        "id": id,
        "name": name,
        "active": active,
        "ordering": ordering,
        "isDefault": is_default,
        "updatedAt": updated_at,
        "deletedAt": deleted_at,
    })))
}

pub async fn update_task(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<TaskUpsert>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let mut tx = state.pool.begin().await?;
    // RETURNING captures the full post-update row inside the tx: the same
    // row is both the PATCH response body and the change-log payload, so
    // pulls can apply REST updates (CloudSyncManager.applyTaskChange
    // requires name+active; a bare {"id"} payload is dropped as sparse).
    let row: Option<(
        String,
        bool,
        Option<i32>,
        bool,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "UPDATE tasks SET \
            name = COALESCE($3, name), \
            active = COALESCE($4, active), \
            ordering = COALESCE($5, ordering), \
            is_default = COALESCE($6, is_default), \
            updated_at = now() \
         WHERE id = $1 AND user_id = $2 \
           AND ($7::timestamptz IS NULL OR updated_at <= $7::timestamptz) \
         RETURNING name, active, ordering, is_default, updated_at, deleted_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(&body.name)
    .bind(body.active)
    .bind(body.ordering)
    .bind(body.is_default)
    .bind(body.updated_at)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((name, active, ordering, is_default, updated_at, deleted_at)) = row else {
        // No row: either the LWW precondition filtered it out, or it's absent.
        let exists: Option<(bool,)> = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = $1 AND user_id = $2) AS e",
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
        "task",
        &id.to_string(),
        "upsert",
        json!({
            "id": id, "name": name, "active": active, "ordering": ordering,
            "isDefault": is_default, "updatedAt": updated_at, "deletedAt": deleted_at,
        }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "id": id,
        "name": name,
        "active": active,
        "ordering": ordering,
        "isDefault": is_default,
        "updatedAt": updated_at,
        "deletedAt": deleted_at,
    })))
}

pub async fn delete_task(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Query(pre): Query<UpdatedAtParam>,
) -> Result<StatusCode, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let pre = pre.updated_at;
    let mut tx = state.pool.begin().await?;
    let res = sqlx::query(
        "UPDATE tasks SET deleted_at = now(), updated_at = now() \
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
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = $1 AND user_id = $2) AS e",
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
        "task",
        &id.to_string(),
        "delete",
        Value::Null,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

//! /targets — target CRUD (OpenAPI tag `targets`).
//!
//! Units (grounded in `DAO`/`TargetEnum`):
//! * `time` — epoch DAYS (a `LocalDate`), stored as the app does
//! * `type` — `DAY_SET`=0, `DAY_GRANT`=1, `DAY_IGNORE`=2, `FLEXI_SET`=5, `FLEXI_ADD`=6
//!   (numeric values exposed via API map to the enum names)
//! * `value` — target work time in MINUTES (nullable on `DAY_IGNORE`)

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::{append_change_tx, date_to_epoch_day, ensure_user, epoch_day_to_date, Auth};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetUpsert {
    /// ISO date (yyyy-MM-dd) — the app's `LocalDate`.
    pub date: NaiveDate,
    pub r#type: String,
    pub value: Option<i32>,
    pub comment: Option<String>,
    /// Accepted (schema-forward-compatible) but unused: the create handler
    /// stamps `now()` server-side; only the PATCH precondition reads it.
    #[allow(dead_code)]
    pub updated_at: Option<DateTime<Utc>>,
}

/// PATCH body: all fields optional (partial update).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetPatch {
    pub date: Option<NaiveDate>,
    pub r#type: Option<String>,
    pub value: Option<i32>,
    pub comment: Option<String>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub include_deleted: Option<bool>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// `updatedAt` query param: optional LWW precondition on deletes.
#[derive(Deserialize)]
pub struct UpdatedAtParam {
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn list_targets(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<ListParams>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let include = q.include_deleted.unwrap_or(false);
    let rows: Vec<(
        Uuid,
        i32,
        String,
        Option<i32>,
        Option<String>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT id, time, type::text, value, comment, updated_at, deleted_at \
         FROM targets WHERE user_id = $1 AND ($2 OR deleted_at IS NULL) \
         AND ($3::int IS NULL OR time >= $3) \
         AND ($4::int IS NULL OR time < $4) \
         ORDER BY time, id LIMIT $5 OFFSET $6",
    )
    .bind(user_id)
    .bind(include)
    .bind(q.from.map(date_to_epoch_day))
    .bind(q.to.map(date_to_epoch_day))
    .bind(q.limit.unwrap_or(500).clamp(1, 1000))
    .bind(q.offset.unwrap_or(0).max(0))
    .fetch_all(&state.pool)
    .await?;
    let targets: Vec<Value> = rows
        .into_iter()
        .map(
            |(id, time, r#type, value, comment, updated_at, deleted_at)| {
                json!({
                    "id": id,
                    "date": epoch_day_to_date(time).format("%Y-%m-%d").to_string(),
                    "type": r#type,
                    "value": value,
                    "comment": comment,
                    "updatedAt": updated_at,
                    "deletedAt": deleted_at,
                })
            },
        )
        .collect();
    Ok(Json(Value::Array(targets)))
}

pub async fn create_target(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<TargetUpsert>,
) -> Result<(StatusCode, Json<Value>), Problem> {
    let ty = body.r#type.as_str();
    if ![
        "DAY_SET",
        "DAY_GRANT",
        "DAY_IGNORE",
        "FLEXI_SET",
        "FLEXI_ADD",
    ]
    .contains(&ty)
    {
        return Err(Problem::validation(format!("invalid target type: {}", ty)));
    }
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let id = Uuid::new_v4();
    let now = Utc::now();
    let date = body.date;
    let epoch_day = date_to_epoch_day(date);
    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO targets (id, user_id, time, type, value, comment, updated_at) \
         VALUES ($1, $2, $3, $4::target_type, $5, $6, $7)",
    )
    .bind(id)
    .bind(user_id)
    .bind(epoch_day)
    .bind(body.r#type.clone())
    .bind(body.value)
    .bind(body.comment.clone())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    append_change_tx(
        &mut tx,
        user_id,
        "target",
        &id.to_string(),
        "upsert",
        json!({ "id": id, "date": date, "type": body.r#type, "value": body.value }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "date": date.format("%Y-%m-%d").to_string(),
            "type": body.r#type,
            "value": body.value,
            "comment": body.comment,
            "updatedAt": now,
            "deletedAt": null,
        })),
    ))
}

pub async fn get_target(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let row: Option<(
        Uuid,
        i32,
        String,
        Option<i32>,
        Option<String>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        "SELECT id, time, type::text, value, comment, updated_at, deleted_at \
         FROM targets WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((id, time, r#type, value, comment, updated_at, deleted_at)) = row else {
        return Err(Problem::not_found());
    };
    Ok(Json(json!({
        "id": id,
        "date": epoch_day_to_date(time).format("%Y-%m-%d").to_string(),
        "type": r#type,
        "value": value,
        "comment": comment,
        "updatedAt": updated_at,
        "deletedAt": deleted_at,
    })))
}

pub async fn update_target(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<TargetPatch>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    if let Some(ty) = body.r#type.as_deref() {
        if ![
            "DAY_SET",
            "DAY_GRANT",
            "DAY_IGNORE",
            "FLEXI_SET",
            "FLEXI_ADD",
        ]
        .contains(&ty)
        {
            return Err(Problem::validation(format!("invalid target type: {}", ty)));
        }
    }
    let mut tx = state.pool.begin().await?;
    // RETURNING captures the full post-update row inside the tx: the same
    // row is both the PATCH response body and the change-log payload, so
    // pulls can apply REST updates (CloudSyncManager.applyTargetChange
    // requires date+type; a bare {"id"} payload is dropped as sparse).
    let row: Option<(
        i32,
        String,
        Option<i32>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "UPDATE targets SET \
            time = COALESCE($3::int, time), \
            type = COALESCE($4::target_type, type), \
            value = COALESCE($5, value), \
            comment = COALESCE($6, comment), \
            updated_at = now() \
         WHERE id = $1 AND user_id = $2 \
           AND ($7::timestamptz IS NULL OR updated_at <= $7::timestamptz) \
         RETURNING time, type::text, value, comment, updated_at, deleted_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(body.date.map(date_to_epoch_day))
    .bind(body.r#type.clone())
    .bind(body.value)
    .bind(body.comment.clone())
    .bind(body.updated_at)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((time, r#type, value, comment, updated_at, deleted_at)) = row else {
        // No row: either the LWW precondition filtered it out, or it's absent.
        let exists: Option<(bool,)> = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM targets WHERE id = $1 AND user_id = $2) AS e",
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
        "target",
        &id.to_string(),
        "upsert",
        json!({
            "id": id, "date": epoch_day_to_date(time).format("%Y-%m-%d").to_string(),
            "type": r#type, "value": value, "comment": comment,
            "updatedAt": updated_at, "deletedAt": deleted_at,
        }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({
        "id": id,
        "date": epoch_day_to_date(time).format("%Y-%m-%d").to_string(),
        "type": r#type,
        "value": value,
        "comment": comment,
        "updatedAt": updated_at,
        "deletedAt": deleted_at,
    })))
}

pub async fn delete_target(
    State(state): State<AppState>,
    auth: Auth,
    Path(id): Path<Uuid>,
    Query(pre): Query<UpdatedAtParam>,
) -> Result<StatusCode, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let pre = pre.updated_at;
    let mut tx = state.pool.begin().await?;
    let res = sqlx::query(
        "UPDATE targets SET deleted_at = now(), updated_at = now() \
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
            "SELECT EXISTS(SELECT 1 FROM targets WHERE id = $1 AND user_id = $2) AS e",
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
        "target",
        &id.to_string(),
        "delete",
        Value::Null,
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

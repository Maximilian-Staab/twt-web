//! /sync — change-log delta sync (OpenAPI tag `sync`).
//!
//! Design §7: pull with `GET /sync/changes?cursor=`, push batches with
//! `POST /sync/push`, state via `GET /sync/state`.
//!
//! Push semantics per op: idempotency via `(client_id, op_id)`; LWW via
//! `updated_at`; validation rejects `CLOCK_OUT_NOW` and negative times.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::{alloc_seq, append_change_tx, ensure_user, Auth};

#[derive(Deserialize)]
pub struct ChangesQuery {
    pub cursor: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn get_changes(
    State(state): State<AppState>,
    auth: Auth,
    Query(q): Query<ChangesQuery>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let cursor = q.cursor.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(500).clamp(1, 1000);

    // REPEATABLE READ so the watermark, page and head come from one
    // consistent snapshot; otherwise a concurrent purge between the reads
    // could hand the client a page that silently skips purged rows.
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *tx)
        .await?;
    let (purged_through_seq,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(purged_through_seq, 0) FROM change_retention WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or((0,));

    // Non-zero cursors that fell inside the purged range get 410: the
    // client cannot tell which of its local rows the missing prefix would
    // have touched, so it must re-baseline (`cursor=0` or `/export`).
    if cursor != 0 && cursor < purged_through_seq {
        return Err(Problem::gone(
            "The requested cursor predates the retained change history.",
        ));
    }

    let rows: Vec<(
        i64,
        String,
        String,
        String,
        Value,
        Option<Uuid>,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT seq, entity, entity_id, op, payload, origin, occurred_at \
         FROM change_log WHERE user_id = $1 AND seq > $2 AND seq > $3 ORDER BY seq LIMIT $4",
    )
    .bind(user_id)
    .bind(cursor)
    .bind(purged_through_seq)
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let has_more = rows.len() as i64 == limit;
    let new_cursor = rows.last().map(|r| r.0).unwrap_or(cursor);

    let changes: Vec<Value> = rows
        .into_iter()
        .map(
            |(seq, entity, entity_id, op, payload, origin, occurred_at)| {
                json!({
                    "seq": seq,
                    "entity": entity,
                    "id": entity_id,
                    "op": op,
                    "data": payload,
                    "origin": origin,
                    "occurredAt": occurred_at,
                })
            },
        )
        .collect();
    Ok(Json(json!({
        "changes": changes,
        "cursor": new_cursor,
        "hasMore": has_more,
    })))
}

/// Purge change-log rows older than `retention_days` and record the
/// per-user high-water mark (`change_retention.purged_through_seq`);
/// `get_changes` answers `410 Gone` for cursors inside the purged range
/// (openapi `/sync/changes` 410, DESIGN §7.1). Returns the number of rows
/// purged. Single statement: delete and watermark upsert commit together,
/// so a reader never sees one without the other.
pub async fn purge_expired(pool: &sqlx::PgPool, retention_days: i64) -> Result<u64, sqlx::Error> {
    let purged: i64 = sqlx::query_scalar(
        "WITH expired AS ( \
            DELETE FROM change_log \
            WHERE occurred_at < now() - make_interval(days => $1) \
            RETURNING user_id, seq \
        ), watermark AS ( \
            INSERT INTO change_retention (user_id, purged_through_seq, purged_at) \
            SELECT user_id, MAX(seq) AS purged_through_seq, now() AS purged_at \
            FROM expired GROUP BY user_id \
            ON CONFLICT (user_id) DO UPDATE SET \
                purged_through_seq = GREATEST(change_retention.purged_through_seq, EXCLUDED.purged_through_seq), \
                purged_at = EXCLUDED.purged_at \
        ) \
        SELECT count(*) FROM expired",
    )
    .bind(retention_days as i32)
    .fetch_one(pool)
    .await?;
    Ok(purged as u64)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    pub client_id: Uuid,
    pub operations: Vec<PushOperation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushOperation {
    pub op_id: Option<String>,
    pub entity: Option<String>,
    pub op: Option<String>,
    pub data: Option<Value>,
}

pub async fn push_batch(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<PushRequest>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    if body.operations.len() > 1000 {
        return Err(Problem::payload_too_large("batch exceeds 1000 operations"));
    }
    // Per the OpenAPI `/clients/{clientId}` description, pushes from revoked
    // clients are rejected with 403; the clientId must also belong to the
    // caller (a foreign clientId is 403, not 404, to avoid leaking existence).
    // The same UPDATE records activity, keeping `last_seen_at` (ordered on by
    // `/sync/state`) current without an extra round trip.
    let touched = sqlx::query(
        "UPDATE clients SET last_seen_at = now() \
         WHERE client_id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(body.client_id)
    .bind(user_id)
    .execute(&state.pool)
    .await?;
    if touched.rows_affected() == 0 {
        return Err(Problem::forbidden(
            "clientId is not an active client of the caller",
        ));
    }
    let mut results: Vec<Value> = Vec::with_capacity(body.operations.len());
    for op in &body.operations {
        let r = apply_one(&state, user_id, body.client_id, op).await?;
        results.push(r);
    }
    let head: Option<(i64,)> = sqlx::query_as("SELECT seq FROM change_seq WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await?;
    Ok(Json(json!({
        "results": results,
        "cursor": head.map(|h| h.0).unwrap_or(0),
    })))
}

async fn apply_one(
    state: &AppState,
    user_id: i64,
    client_id: Uuid,
    op: &PushOperation,
) -> Result<Value, Problem> {
    let op_id = op.op_id.clone().unwrap_or_default();
    if op_id.is_empty() {
        return Ok(json!({ "opId": op_id, "result": "rejected", "reason": "missing opId" }));
    }
    // idempotency: (client_id, op_id) seen => duplicate. Checked outside the
    // tx: a concurrent duplicate would serialize on the ledger PK anyway.
    let seen: Option<(String,)> =
        sqlx::query_as("SELECT result FROM processed_ops WHERE client_id = $1 AND op_id = $2")
            .bind(client_id)
            .bind(&op_id)
            .fetch_optional(&state.pool)
            .await?;
    if seen.is_some() {
        // DESIGN §7.2: already processed → duplicate, no state change
        return Ok(json!({ "opId": op_id, "result": "duplicate" }));
    }
    let entity = op.entity.clone().unwrap_or_default();
    let op_kind = op.op.clone().unwrap_or_default();
    let data = op.data.clone().unwrap_or(Value::Null);

    // Entity write, change-log append and ledger write are one transaction:
    // a crash between them can no longer leave a written row without its
    // change-log entry (or vice versa), which would break delta sync.
    let mut tx = state.pool.begin().await?;
    let (apply_result, change_payload) =
        apply_entity_op(&mut tx, user_id, &entity, &op_kind, &data).await?;
    let result_json = if apply_result == "applied" {
        append_change_tx(
            &mut tx,
            user_id,
            &entity,
            &extract_id(&data),
            &op_kind,
            data,
            Some(client_id),
        )
        .await?;
        mark_processed(&mut tx, client_id, &op_id, "applied").await?;
        json!({ "opId": op_id, "result": "applied" })
    } else {
        // rejected / ignoredStale carry a reason in the payload
        mark_processed(&mut tx, client_id, &op_id, &apply_result).await?;
        let reason = change_payload.get("reason").cloned().unwrap_or(Value::Null);
        json!({ "opId": op_id, "result": apply_result, "reason": reason })
    };
    tx.commit().await?;
    Ok(result_json)
}

fn extract_id(data: &Value) -> String {
    data.get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

async fn mark_processed(
    conn: &mut sqlx::PgConnection,
    client_id: Uuid,
    op_id: &str,
    result: &str,
) -> Result<(), Problem> {
    sqlx::query("INSERT INTO processed_ops (client_id, op_id, result) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(client_id)
        .bind(op_id)
        .bind(result)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Applies an op to the entity table. Returns ("applied"|"ignoredStale"|"rejected", payload).
async fn apply_entity_op(
    conn: &mut sqlx::PgConnection,
    user_id: i64,
    entity: &str,
    op_kind: &str,
    data: &Value,
) -> Result<(String, Value), Problem> {
    let id = data
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Ok(uuid) = Uuid::parse_str(&id) else {
        return Ok(("rejected".into(), json!({"reason": "invalid id"})));
    };
    match entity {
        "event" => {
            let ty = data.get("type").and_then(Value::as_str).unwrap_or_default();
            if ty != "CLOCK_IN" && ty != "CLOCK_OUT" {
                return Ok((
                    "rejected".into(),
                    json!({"reason": "CLOCK_OUT_NOW or unknown type rejected"}),
                ));
            }
            let time = data.get("time").and_then(Value::as_i64);
            let Some(time) = time else {
                return Ok(("rejected".into(), json!({"reason": "missing time"})));
            };
            if time < 0 {
                return Ok(("rejected".into(), json!({"reason": "negative time"})));
            }
            let zone = data.get("zoneOffset").and_then(Value::as_i64).unwrap_or(0);
            if !crate::routes::zone_offset_ok(zone) {
                return Ok((
                    "rejected".into(),
                    json!({"reason": "zoneOffset out of range (±1080 min)"}),
                ));
            }
            let task = data
                .get("task")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());
            let comment = data.get("comment").and_then(Value::as_str);
            let updated_at = data
                .get("updatedAt")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            let updated_at = updated_at.unwrap_or_else(|| chrono::Utc::now().fixed_offset());
            if op_kind == "delete" {
                sqlx::query("UPDATE events SET deleted_at = $3, updated_at = $4 WHERE id = $1 AND user_id = $2")
                    .bind(uuid).bind(user_id).bind(updated_at).bind(updated_at)
                    .execute(&mut *conn).await?;
                return Ok(("applied".into(), data.clone()));
            }
            // LWW: a stored row with a strictly newer updatedAt wins (DESIGN §7.2);
            // equal timestamps apply (idempotent replay of the same op).
            let stored_updated: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
                sqlx::query_as("SELECT updated_at FROM events WHERE id = $1 AND user_id = $2")
                    .bind(uuid)
                    .bind(user_id)
                    .fetch_optional(&mut *conn)
                    .await?;
            if let Some((stored,)) = stored_updated {
                let stored = stored.unwrap_or_else(chrono::Utc::now);
                if stored > updated_at {
                    return Ok((
                        "ignoredStale".into(),
                        json!({"reason": "stored row is newer", "current": stored.to_rfc3339()}),
                    ));
                }
            }
            let insertion_seq = alloc_seq(&mut *conn, user_id).await?;
            let res = sqlx::query(
                "INSERT INTO events (id, user_id, time, zone_offset, type, task, comment, updated_at, insertion_seq) \
                 VALUES ($1, $2, $3, $4, $5::event_type, $6, $7, $8, $9) \
                 ON CONFLICT (id) DO UPDATE SET \
                   time = EXCLUDED.time, zone_offset = EXCLUDED.zone_offset, \
                   type = EXCLUDED.type, task = EXCLUDED.task, comment = EXCLUDED.comment, \
                   updated_at = EXCLUDED.updated_at, deleted_at = NULL \
                 WHERE events.user_id = $2",
            )
            .bind(uuid)
            .bind(user_id)
            .bind(time)
            .bind(zone as i32)
            .bind(ty)
            .bind(task)
            .bind(comment)
            .bind(updated_at)
            .bind(insertion_seq)
            .execute(&mut *conn)
            .await?;
            if res.rows_affected() == 0 {
                // Conflicting row belongs to another user: the DO UPDATE guard
                // suppressed the write entirely. Reject instead of leaking.
                return Ok((
                    "rejected".into(),
                    json!({"reason": "id belongs to another user"}),
                ));
            }
            Ok(("applied".into(), data.clone()))
        }
        "task" => {
            let name = data.get("name").and_then(Value::as_str).unwrap_or_default();
            let active = data.get("active").and_then(Value::as_bool).unwrap_or(false);
            let ordering = data.get("ordering").and_then(Value::as_i64);
            let is_default = data
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let updated_at = data
                .get("updatedAt")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .unwrap_or_else(|| chrono::Utc::now().fixed_offset());
            if op_kind == "delete" {
                sqlx::query("UPDATE tasks SET deleted_at = $3, updated_at = $4 WHERE id = $1 AND user_id = $2")
                    .bind(uuid).bind(user_id).bind(updated_at).bind(updated_at)
                    .execute(&mut *conn).await?;
                return Ok(("applied".into(), data.clone()));
            }
            let res = sqlx::query(
                "INSERT INTO tasks (id, user_id, name, active, ordering, is_default, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7) \
                 ON CONFLICT (id) DO UPDATE SET \
                   name = EXCLUDED.name, active = EXCLUDED.active, ordering = EXCLUDED.ordering, \
                   is_default = EXCLUDED.is_default, updated_at = EXCLUDED.updated_at, deleted_at = NULL \
                 WHERE tasks.user_id = $2",
            )
            .bind(uuid).bind(user_id).bind(name).bind(active).bind(ordering).bind(is_default).bind(updated_at)
            .execute(&mut *conn)
            .await?;
            if res.rows_affected() == 0 {
                return Ok((
                    "rejected".into(),
                    json!({"reason": "id belongs to another user"}),
                ));
            }
            Ok(("applied".into(), data.clone()))
        }
        "target" => {
            let date = data
                .get("date")
                .and_then(Value::as_str)
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            let Some(date) = date else {
                return Ok(("rejected".into(), json!({"reason": "missing/invalid date"})));
            };
            let ty = data.get("type").and_then(Value::as_str).unwrap_or_default();
            if ![
                "DAY_SET",
                "DAY_GRANT",
                "DAY_IGNORE",
                "FLEXI_SET",
                "FLEXI_ADD",
            ]
            .contains(&ty)
            {
                return Ok(("rejected".into(), json!({"reason": "invalid target type"})));
            }
            let value = data.get("value").and_then(Value::as_i64);
            let comment = data.get("comment").and_then(Value::as_str);
            let updated_at = data
                .get("updatedAt")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .unwrap_or_else(|| chrono::Utc::now().fixed_offset());
            if op_kind == "delete" {
                sqlx::query("UPDATE targets SET deleted_at = $3, updated_at = $4 WHERE id = $1 AND user_id = $2")
                    .bind(uuid).bind(user_id).bind(updated_at).bind(updated_at)
                    .execute(&mut *conn).await?;
                return Ok(("applied".into(), data.clone()));
            }
            let res = sqlx::query(
                "INSERT INTO targets (id, user_id, time, type, value, comment, updated_at) \
                 VALUES ($1, $2, $3, $4::target_type, $5, $6, $7) \
                 ON CONFLICT (id) DO UPDATE SET time = EXCLUDED.time, \
                   type = EXCLUDED.type, value = EXCLUDED.value, comment = EXCLUDED.comment, \
                   updated_at = EXCLUDED.updated_at, deleted_at = NULL \
                 WHERE targets.user_id = $2",
            )
            .bind(uuid)
            .bind(user_id)
            .bind(crate::routes::date_to_epoch_day(date))
            .bind(ty)
            .bind(value)
            .bind(comment)
            .bind(updated_at)
            .execute(&mut *conn)
            .await?;
            if res.rows_affected() == 0 {
                return Ok((
                    "rejected".into(),
                    json!({"reason": "id belongs to another user"}),
                ));
            }
            Ok(("applied".into(), data.clone()))
        }
        "preference" => {
            let key = data
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let value = data.get("value").cloned().unwrap_or(Value::Null);
            let updated_at = chrono::Utc::now();
            sqlx::query(
                "INSERT INTO preferences (user_id, key, value, updated_at) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at",
            )
            .bind(user_id).bind(&key).bind(&value).bind(updated_at)
            .execute(&mut *conn).await?;
            Ok(("applied".into(), data.clone()))
        }
        _ => Ok(("rejected".into(), json!({"reason": "unknown entity"}))),
    }
}

pub async fn sync_state(State(state): State<AppState>, auth: Auth) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let head: Option<(i64,)> = sqlx::query_as("SELECT seq FROM change_seq WHERE user_id = $1")
        .bind(user_id)
        .fetch_optional(&state.pool)
        .await?;
    let cursor = head.map(|h| h.0).unwrap_or(0);
    let clients: Vec<(Uuid, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT client_id, last_seen_at FROM clients WHERE user_id = $1 AND revoked_at IS NULL ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    let clients: Vec<Value> = clients
        .into_iter()
        .map(|(id, last_seen)| json!({ "clientId": id, "lastSeenAt": last_seen }))
        .collect();
    Ok(Json(json!({ "cursor": cursor, "clients": clients })))
}

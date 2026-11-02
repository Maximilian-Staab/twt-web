//! /backup/* — CSV import/export byte-compatible with the app's backup format.
//!
//! Ground truth (TWT app):
//! * `DAO.backupEventsToWriter` + `DAO.restoreEventsFromReader`
//! * `DAO.backupTargetsToWriter` + `DAO.restoreTargetsFromReader`
//! * `PreferencesUtil.writePreferences` / `readPreferences`

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, FixedOffset, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::{alloc_seq, ensure_user, Auth};

const CSV_EOL: &str = "\n";

fn event_csv_type_name(ty: &str) -> &'static str {
    match ty {
        "CLOCK_IN" => "in",
        "CLOCK_OUT" => "out",
        _ => "",
    }
}

fn target_type_from_name(name: &str) -> Option<&'static str> {
    match name {
        "day-set" => Some("DAY_SET"),
        "day-grant" => Some("DAY_GRANT"),
        "day-ignore" => Some("DAY_IGNORE"),
        "flexi-set" => Some("FLEXI_SET"),
        "flexi-add" => Some("FLEXI_ADD"),
        _ => None,
    }
}

fn target_type_to_name(ty: &str) -> &'static str {
    match ty {
        "DAY_SET" => "day-set",
        "DAY_GRANT" => "day-grant",
        "DAY_IGNORE" => "day-ignore",
        "FLEXI_SET" => "flexi-set",
        "FLEXI_ADD" => "flexi-add",
        _ => "day-set",
    }
}

/// DAO.backupEventsToWriter replaces [\r\n] with ' ' and ';' with ','.
fn sanitize_comment(comment: Option<&str>) -> String {
    match comment {
        None => String::new(),
        Some(c) => c
            .chars()
            .map(|ch| match ch {
                '\r' | '\n' => ' ',
                ';' => ',',
                _ => ch,
            })
            .collect(),
    }
}

fn iso_offset_date_time(epoch_seconds: i64, zone_offset_minutes: i32) -> String {
    // Clamp legacy out-of-range offsets to the Java ZoneOffset bound (±18h);
    // `east_opt` returns None outside it. A panic here kills the connection,
    // so degrade to UTC instead of unwrapping.
    let secs = (zone_offset_minutes as i64) * 60;
    let clamped = secs.clamp(-18 * 3600, 18 * 3600);
    let offset = FixedOffset::east_opt(clamped as i32).expect("clamped to ±18h, always valid");
    let dt = DateTime::<Utc>::from_timestamp(epoch_seconds, 0).expect("valid epoch seconds");
    dt.with_timezone(&offset)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

// ---------------------------------------------------------------------------
// events export
// ---------------------------------------------------------------------------

/// Per-user, 1-based task numbering for the backup CSV: reuses `local_id`
/// when set, otherwise derives the next number above the current max in
/// `updated_at` order. Derived numbers are never written back — export is a
/// pure read, so concurrent exports cannot race each other.
async fn export_task_numbering(
    conn: &mut sqlx::PgConnection,
    user_id: i64,
) -> Result<BTreeMap<i32, (Uuid, String, bool, Option<i32>, bool)>, Problem> {
    let rows: Vec<(Uuid, Option<i32>, String, bool, Option<i32>, bool)> = sqlx::query_as(
        "SELECT id, local_id, name, active, ordering, is_default \
         FROM tasks WHERE user_id = $1 AND deleted_at IS NULL \
         ORDER BY local_id NULLS LAST, updated_at, id",
    )
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut next = rows
        .iter()
        .filter_map(|(_, local_id, ..)| *local_id)
        .max()
        .unwrap_or(0);
    // BTreeMap keeps the task-only rows in ascending `local_id` order,
    // independent of hash seed.
    let mut map: BTreeMap<i32, (Uuid, String, bool, Option<i32>, bool)> = BTreeMap::new();
    for (id, local_id, name, active, ordering, is_default) in rows {
        let num = match local_id {
            Some(n) => n,
            None => {
                next = next
                    .checked_add(1)
                    .ok_or_else(|| Problem::internal("task numbering exhausted"))?;
                next
            }
        };
        map.insert(num, (id, name, active, ordering, is_default));
    }
    Ok(map)
}

pub async fn backup_events_get(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Response, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    // One REPEATABLE READ snapshot keeps tasks and events consistent: an
    // event's task is always visible in the same export that lists the event.
    let mut tx = state.pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let task_map = export_task_numbering(&mut tx, user_id).await?;
    let mut task_by_uuid: HashMap<Uuid, i32> = HashMap::new();
    for (num, (id, _, _, _, _)) in &task_map {
        task_by_uuid.insert(*id, *num);
    }

    let events: Vec<(Uuid, i64, i32, String, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT id, time, zone_offset, type::text, task, comment \
         FROM events WHERE user_id = $1 AND deleted_at IS NULL ORDER BY time, id",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let mut csv = String::new();
    csv.push_str("type;time;task;comment;taskId;name;active;ordering;isdefault");
    csv.push_str(CSV_EOL);

    let mut tasks_with_events: std::collections::HashSet<i32> = std::collections::HashSet::new();
    for (_id, time, zone_offset, ty, task, comment) in &events {
        let num = task
            .as_ref()
            .and_then(|t| task_by_uuid.get(t))
            .copied()
            .unwrap_or(0);
        tasks_with_events.insert(num);
        let _ = write!(
            csv,
            "{};{};{};{};",
            event_csv_type_name(ty),
            iso_offset_date_time(*time, *zone_offset),
            num,
            sanitize_comment(comment.as_deref())
        );
        if let Some((_, name, active, ordering, is_default)) = task_map.get(&num) {
            let _ = write!(
                csv,
                "{};{};{};{};{};",
                num,
                name,
                *active as i32,
                ordering.unwrap_or(0),
                *is_default as i32
            );
        } else {
            csv.push_str(";;;;;");
        }
        csv.push_str(CSV_EOL);
    }
    // tasks without events: `;;;;` + task columns
    for (num, (_, name, active, ordering, is_default)) in &task_map {
        if !tasks_with_events.contains(num) {
            let _ = write!(
                csv,
                ";;;;{};{};{};{};{};",
                num,
                name,
                *active as i32,
                ordering.unwrap_or(0),
                *is_default as i32
            );
            csv.push_str(CSV_EOL);
        }
    }

    Ok((
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        csv.into_bytes(),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// events import
// ---------------------------------------------------------------------------

pub async fn backup_events_put(
    State(state): State<AppState>,
    auth: Auth,
    body: String,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut lines = body.lines();
    let _header = lines.next();
    let mut task_by_local: HashMap<i32, Uuid> = HashMap::new();

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(';').collect();
        // cols[0] is the event type ("" for task-only rows)
        // cols[4] is taskId ("" for events without task)
        if cols.len() < 5 {
            skipped += 1;
            continue;
        }
        // task columns (5..9): mirror DAO.restoreEventsFromReader, which only
        // imports the task when the row has all of them (length > 8)
        if cols.len() > 8 && !cols[4].is_empty() {
            let Ok(local_id) = cols[4].parse::<i32>() else {
                skipped += 1;
                continue;
            };
            let name = cols[5].to_string();
            let active = cols[6].parse::<i32>().unwrap_or(0) != 0;
            let ordering = cols[7].parse::<i32>().ok();
            let is_default = cols[8].parse::<i32>().unwrap_or(0) != 0;
            let uuid = import_task(
                &state.pool,
                user_id,
                local_id,
                &name,
                active,
                ordering,
                is_default,
            )
            .await?;
            task_by_local.insert(local_id, uuid);
        }
        if !cols[0].is_empty() {
            // parse the event time
            let Ok(dt) = chrono::DateTime::parse_from_rfc3339(cols[1]) else {
                skipped += 1;
                continue;
            };
            let task_uuid = cols[4]
                .parse::<i32>()
                .ok()
                .and_then(|l| task_by_local.get(&l).copied());
            let comment = if cols[3].is_empty() {
                None
            } else {
                Some(cols[3].to_string())
            };
            let ty = if cols[0] == "in" {
                "CLOCK_IN"
            } else {
                "CLOCK_OUT"
            };
            insert_event(
                &state.pool,
                user_id,
                dt.timestamp(),
                dt.offset().local_minus_utc() / 60,
                ty,
                task_uuid,
                comment,
            )
            .await?;
        }
        imported += 1;
    }
    Ok(Json(json!({ "imported": imported, "skipped": skipped })))
}

async fn import_task(
    pool: &sqlx::PgPool,
    user_id: i64,
    local_id: i32,
    name: &str,
    active: bool,
    ordering: Option<i32>,
    is_default: bool,
) -> Result<Uuid, Problem> {
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM tasks WHERE user_id = $1 AND local_id = $2")
            .bind(user_id)
            .bind(local_id)
            .fetch_optional(pool)
            .await?;
    if let Some((id,)) = existing {
        return Ok(id);
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tasks (id, user_id, name, active, ordering, is_default, updated_at, local_id) \
         VALUES ($1, $2, $3, $4, $5, $6, now(), $7)",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(active)
    .bind(ordering)
    .bind(is_default)
    .bind(local_id)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn insert_event(
    pool: &sqlx::PgPool,
    user_id: i64,
    time: i64,
    zone_offset: i32,
    ty: &str,
    task: Option<Uuid>,
    comment: Option<String>,
) -> Result<(), Problem> {
    // dedupe by natural key: same (user, time, type, task) => same event.
    // The app's restore deletes all first; upsert achieves the same result
    // without destroying concurrent JSON-API writes.
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM events WHERE user_id = $1 AND time = $2 AND type = $3::event_type AND task IS NOT DISTINCT FROM $4",
    )
    .bind(user_id)
    .bind(time)
    .bind(ty)
    .bind(task)
    .fetch_optional(pool)
    .await?;
    if let Some((id,)) = existing {
        // refresh zone/comment (LWW: the CSV row is the newest truth)
        sqlx::query(
            "UPDATE events SET zone_offset = $2, comment = $3, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(zone_offset)
        .bind(comment)
        .execute(pool)
        .await?;
        return Ok(());
    }
    let id = Uuid::new_v4();
    let insertion_seq = alloc_seq(pool, user_id).await?;
    sqlx::query(
        "INSERT INTO events (id, user_id, time, zone_offset, type, task, comment, updated_at, insertion_seq) \
         VALUES ($1, $2, $3, $4, $5::event_type, $6, $7, now(), $8)",
    )
    .bind(id)
    .bind(user_id)
    .bind(time)
    .bind(zone_offset)
    .bind(ty)
    .bind(task)
    .bind(comment)
    .bind(insertion_seq)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// targets export
// ---------------------------------------------------------------------------

pub async fn backup_targets_get(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Response, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let rows: Vec<(i32, String, Option<i32>, Option<String>)> = sqlx::query_as(
        "SELECT time, type::text, value, comment FROM targets WHERE user_id = $1 AND deleted_at IS NULL ORDER BY time",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;
    let mut csv = String::new();
    csv.push_str("time;type;value;comment");
    csv.push_str(CSV_EOL);
    for (days, ty, value, comment) in rows {
        let date = crate::routes::epoch_day_to_date(days);
        let _ = writeln!(
            csv,
            "{};{};{};{}",
            date.format("%Y-%m-%d"),
            target_type_to_name(&ty),
            value.unwrap_or(0),
            comment.unwrap_or_default(),
        );
    }
    Ok((
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        csv.into_bytes(),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// targets import
// ---------------------------------------------------------------------------

pub async fn backup_targets_put(
    State(state): State<AppState>,
    auth: Auth,
    body: String,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut lines = body.lines();
    let _header = lines.next();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split(';').collect();
        if cols.len() < 4 || cols[0].is_empty() {
            skipped += 1;
            continue;
        }
        let Ok(date) = chrono::NaiveDate::parse_from_str(cols[0], "%Y-%m-%d") else {
            skipped += 1;
            continue;
        };
        let Some(ty) = target_type_from_name(cols[1]) else {
            skipped += 1;
            continue;
        };
        let value: i32 = cols[2].parse().unwrap_or(0);
        let comment = if cols[3].is_empty() {
            None
        } else {
            Some(cols[3].to_string())
        };
        import_target(&state.pool, user_id, date, ty, value, comment).await?;
        imported += 1;
    }
    Ok(Json(json!({ "imported": imported, "skipped": skipped })))
}

/// Insert or fetch a target keyed by `(user_id, time, type)` (LWW by updated_at).
async fn import_target(
    pool: &sqlx::PgPool,
    user_id: i64,
    date: chrono::NaiveDate,
    ty: &str,
    value: i32,
    comment: Option<String>,
) -> Result<Uuid, Problem> {
    let days = crate::routes::date_to_epoch_day(date);
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM targets WHERE user_id = $1 AND time = $2 AND type = $3::target_type",
    )
    .bind(user_id)
    .bind(days)
    .bind(ty)
    .fetch_optional(pool)
    .await?;
    if let Some((id,)) = existing {
        sqlx::query(
            "UPDATE targets SET value = $2, comment = $3, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(value)
        .bind(comment)
        .execute(pool)
        .await?;
        return Ok(id);
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO targets (id, user_id, time, type, value, comment, updated_at) \
         VALUES ($1, $2, $3, $4::target_type, $5, $6, now())",
    )
    .bind(id)
    .bind(user_id)
    .bind(days)
    .bind(ty)
    .bind(value)
    .bind(comment)
    .execute(pool)
    .await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// preferences export
// ---------------------------------------------------------------------------

pub async fn backup_prefs_get(
    State(state): State<AppState>,
    auth: Auth,
) -> Result<Response, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT key, value FROM preferences WHERE user_id = $1 ORDER BY key")
            .bind(user_id)
            .fetch_all(&state.pool)
            .await?;
    let mut csv = String::new();
    for (key, value) in rows {
        // JSON-encoded values are typed; export as key;type;value
        let (ty, rendered) = match &value {
            Value::String(s) => ("string", s.clone()),
            Value::Bool(b) => ("boolean", b.to_string()),
            Value::Number(n) if n.is_i64() || n.is_u64() => ("integer", n.to_string()),
            Value::Number(n) => ("float", n.to_string()),
            Value::Array(a) => (
                "stringset",
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join("|"),
            ),
            _ => ("string", value.to_string()),
        };
        let _ = write!(csv, "{};{};{}", key, ty, rendered);
        csv.push_str(CSV_EOL);
    }
    Ok((
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        csv.into_bytes(),
    )
        .into_response())
}

pub async fn backup_prefs_put(
    State(state): State<AppState>,
    auth: Auth,
    body: String,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let mut imported = 0usize;
    let mut skipped = 0usize;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.splitn(3, ';').collect();
        if cols.len() < 3 {
            skipped += 1;
            continue;
        }
        let value = match cols[1] {
            "string" => Value::String(cols[2].to_string()),
            "stringset" => Value::Array(
                cols[2]
                    .split('|')
                    .map(|s| Value::String(s.to_string()))
                    .collect(),
            ),
            "boolean" => Value::Bool(cols[2].parse().unwrap_or(false)),
            "integer" | "long" => match cols[2].parse::<i64>() {
                Ok(n) => Value::Number(n.into()),
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            },
            "float" => match cols[2].parse::<f64>() {
                Ok(f) => {
                    Value::Number(serde_json::Number::from_f64(f).unwrap_or_else(|| 0i64.into()))
                }
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            },
            _ => {
                skipped += 1;
                continue;
            }
        };
        sqlx::query(
            "INSERT INTO preferences (user_id, key, value, updated_at) VALUES ($1, $2, $3, now()) \
             ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(user_id)
        .bind(cols[0])
        .bind(&value)
        .execute(&state.pool)
        .await?;
        imported += 1;
    }
    Ok(Json(json!({ "imported": imported, "skipped": skipped })))
}

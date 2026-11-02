//! /export, /import — full-state JSON snapshot (OpenAPI tag `snapshot`).
//!
//! `Snapshot` schema: `{ tasks: [...], events: [...], targets: [...],
//! preferences: {...} }` — see openapi.yaml.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::api::AppState;
use crate::error::Problem;
use crate::routes::{alloc_seq, ensure_user, epoch_day_to_date, Auth};

pub async fn export_all(State(state): State<AppState>, auth: Auth) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;

    let tasks: Vec<(
        Uuid,
        String,
        bool,
        Option<i32>,
        bool,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "SELECT id, name, active, ordering, is_default, updated_at, deleted_at \
                       FROM tasks WHERE user_id = $1 ORDER BY name",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;

    let events: Vec<(
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
                       FROM events WHERE user_id = $1 ORDER BY time",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;

    let targets: Vec<(
        Uuid,
        i32,
        String,
        Option<i32>,
        Option<String>,
        chrono::DateTime<chrono::Utc>,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as(
        "SELECT id, time, type::text, value, comment, updated_at, deleted_at \
                       FROM targets WHERE user_id = $1 ORDER BY time",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await?;

    let prefs: Vec<(String, Value)> =
        sqlx::query_as("SELECT key, value FROM preferences WHERE user_id = $1 ORDER BY key")
            .bind(user_id)
            .fetch_all(&state.pool)
            .await?;

    let tasks_json: Vec<Value> = tasks
        .into_iter()
        .map(
            |(id, name, active, ordering, is_default, updated_at, deleted_at)| {
                json!({
                    "id": id, "name": name, "active": active, "ordering": ordering,
                    "isDefault": is_default, "updatedAt": updated_at, "deletedAt": deleted_at,
                })
            },
        )
        .collect();
    let events_json: Vec<Value> = events
        .into_iter()
        .map(|(id, time, zone_offset, ty, task, comment, updated_at, deleted_at)| {
            json!({
                "id": id, "time": time, "zoneOffset": zone_offset, "type": ty,
                "task": task, "comment": comment, "updatedAt": updated_at, "deletedAt": deleted_at,
            })
        })
        .collect();
    let targets_json: Vec<Value> = targets
        .into_iter()
        .map(|(id, days, ty, value, comment, updated_at, deleted_at)| {
            let date = epoch_day_to_date(days).format("%Y-%m-%d").to_string();
            json!({
                "id": id, "date": date, "type": ty, "value": value, "comment": comment,
                "updatedAt": updated_at, "deletedAt": deleted_at,
            })
        })
        .collect();
    let mut prefs_map = Map::new();
    for (key, value) in prefs {
        prefs_map.insert(key, value);
    }
    Ok(Json(json!({
        "tasks": tasks_json,
        "events": events_json,
        "targets": targets_json,
        "preferences": Value::Object(prefs_map),
    })))
}

pub async fn import_all(
    State(state): State<AppState>,
    auth: Auth,
    Json(body): Json<Value>,
) -> Result<Json<Value>, Problem> {
    let user_id = ensure_user(&state.pool, &auth.0.sub).await?;
    let mut imported = 0;
    let mut skipped = 0;

    // Tasks
    if let Some(tasks) = body.get("tasks").and_then(Value::as_array) {
        for task in tasks {
            let id = task
                .get("id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());
            let Some(id) = id else {
                skipped += 1;
                continue;
            };
            let name = task.get("name").and_then(Value::as_str).unwrap_or("");
            let active = task.get("active").and_then(Value::as_bool).unwrap_or(false);
            let ordering = task.get("ordering").and_then(Value::as_i64);
            let is_default = task
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let updated_at = task
                .get("updatedAt")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            let res = sqlx::query(
                "INSERT INTO tasks (id, user_id, name, active, ordering, is_default, updated_at) \
                 VALUES ($1, $2, $3, $4, $5::int, $6, $7) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(id)
            .bind(user_id)
            .bind(name)
            .bind(active)
            .bind(ordering)
            .bind(is_default)
            .bind(updated_at.unwrap_or_else(|| chrono::Utc::now().fixed_offset()))
            .execute(&state.pool)
            .await?;
            // Suppressed by ON CONFLICT DO NOTHING: the id already exists
            // (own row re-imported, or another user's row). Unchanged → skipped.
            if res.rows_affected() == 0 {
                skipped += 1;
            } else {
                imported += 1;
            }
        }
    }

    // Events
    if let Some(events) = body.get("events").and_then(Value::as_array) {
        for event in events {
            let id = event
                .get("id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());
            let Some(id) = id else {
                skipped += 1;
                continue;
            };
            let time = event.get("time").and_then(Value::as_i64);
            let zone = event.get("zoneOffset").and_then(Value::as_i64);
            let ty = event.get("type").and_then(Value::as_str);
            let task = event
                .get("task")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());
            let comment = event.get("comment").and_then(Value::as_str);
            let updated_at = event
                .get("updatedAt")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            if time.is_none() || zone.is_none() || ty.is_none() {
                skipped += 1;
                continue;
            }
            // Import is a bulk migration path: clamp negative times to the
            // epoch (EventUpsert declares minimum: 0) rather than drop rows.
            let time = time.unwrap().max(0);
            if !crate::routes::zone_offset_ok(zone.unwrap()) {
                skipped += 1;
                continue;
            }
            if !["CLOCK_IN", "CLOCK_OUT"].contains(&ty.unwrap()) {
                skipped += 1;
                continue;
            }
            let res = sqlx::query(
                "INSERT INTO events (id, user_id, time, zone_offset, type, task, comment, updated_at, insertion_seq) \
                 VALUES ($1, $2, $3, $4, $5::event_type, $6, $7, $8, $9) ON CONFLICT (id) DO NOTHING",
            )
            .bind(id)
            .bind(user_id)
            .bind(time)
            .bind(zone.unwrap() as i32)
            .bind(ty.unwrap())
            .bind(task)
            .bind(comment)
            .bind(updated_at.unwrap_or_else(|| chrono::Utc::now().fixed_offset()))
            .bind(alloc_seq(&state.pool, user_id).await?)
            .execute(&state.pool)
            .await?;
            // Suppressed by ON CONFLICT DO NOTHING: the id already exists
            // (own row re-imported, or another user's row). Unchanged → skipped.
            if res.rows_affected() == 0 {
                skipped += 1;
            } else {
                imported += 1;
            }
        }
    }

    // Targets
    if let Some(targets) = body.get("targets").and_then(Value::as_array) {
        for target in targets {
            let id = target
                .get("id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());
            let Some(id) = id else {
                skipped += 1;
                continue;
            };
            let date = target
                .get("date")
                .and_then(Value::as_str)
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            let ty = target.get("type").and_then(Value::as_str);
            let value = target.get("value").and_then(Value::as_i64);
            let comment = target.get("comment").and_then(Value::as_str);
            let updated_at = target
                .get("updatedAt")
                .and_then(Value::as_str)
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
            if date.is_none() || ty.is_none() {
                skipped += 1;
                continue;
            }
            if ![
                "DAY_SET",
                "DAY_GRANT",
                "DAY_IGNORE",
                "FLEXI_SET",
                "FLEXI_ADD",
            ]
            .contains(&ty.unwrap_or(""))
            {
                skipped += 1;
                continue;
            }
            let res = sqlx::query(
                "INSERT INTO targets (id, user_id, time, type, value, comment, updated_at) \
                 VALUES ($1, $2, $3, $4::target_type, $5, $6, $7) ON CONFLICT (id) DO NOTHING",
            )
            .bind(id)
            .bind(user_id)
            .bind(crate::routes::date_to_epoch_day(date.unwrap()))
            .bind(ty)
            .bind(value)
            .bind(comment)
            .bind(updated_at.unwrap_or_else(|| chrono::Utc::now().fixed_offset()))
            .execute(&state.pool)
            .await?;
            // Suppressed by ON CONFLICT DO NOTHING: the id already exists
            // (own row re-imported, or another user's row). Unchanged → skipped.
            if res.rows_affected() == 0 {
                skipped += 1;
            } else {
                imported += 1;
            }
        }
    }

    // Preferences
    if let Some(prefs) = body.get("preferences").and_then(Value::as_object) {
        for (key, value) in prefs {
            let res = sqlx::query(
                "INSERT INTO preferences (user_id, key, value, updated_at) VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (user_id, key) DO NOTHING",
            )
            .bind(user_id)
            .bind(key)
            .bind(value)
            .bind(chrono::Utc::now())
            .execute(&state.pool)
            .await?;
            // Suppressed by ON CONFLICT DO NOTHING: the pref already exists.
            // Unchanged → skipped.
            if res.rows_affected() == 0 {
                skipped += 1;
            } else {
                imported += 1;
            }
        }
    }

    Ok(Json(json!({ "imported": imported, "skipped": skipped })))
}

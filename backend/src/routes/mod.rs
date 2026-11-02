//! Route handlers, grouped by OpenAPI tag. See
//! `docs/openapi.yaml` (normative contract).
//!
//! Units are grounded in the app (`DAO`, `MySQLiteHelper` v6):
//! * event `time` epoch SECONDS; `zone_offset` UTC offset in MINUTES
//! * target `time` epoch DAYS (a `LocalDate`); `value` target work MINUTES.

pub mod backup;
pub mod clients;
pub mod config;
pub mod events;
pub mod identity;
pub mod preferences;
pub mod snapshot;
pub mod sync;
pub mod targets;
pub mod tasks;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde_json::Value;
use uuid::Uuid;

use crate::api::AppState;
use crate::auth::Claims;
use crate::error::Problem;

// ---------------------------------------------------------------------------
// auth extractor
// ---------------------------------------------------------------------------

pub struct Auth(pub Claims);

impl FromRequestParts<AppState> for Auth {
    type Rejection = Problem;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| Problem::unauthorized("missing Authorization header"))?;
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or_else(|| Problem::unauthorized("expected Bearer scheme"))?;
        let claims = crate::auth::validate_token(token, &state.auth).await?;
        Ok(Auth(claims))
    }
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

/// Ensures a `users` row exists for the OIDC `sub`; returns its id.
pub async fn ensure_user(pool: &sqlx::PgPool, sub: &str) -> Result<i64, Problem> {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO users (sub) VALUES ($1) ON CONFLICT (sub) DO UPDATE SET sub = EXCLUDED.sub RETURNING id",
    )
    .bind(sub)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Allocates the next per-user change-log seq atomically. Executor-generic so
/// callers can run inside a transaction.
pub async fn next_seq<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
    user_id: i64,
) -> Result<i64, Problem> {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO change_seq (user_id, seq) VALUES ($1, 1) \
         ON CONFLICT (user_id) DO UPDATE SET seq = change_seq.seq + 1 RETURNING seq",
    )
    .bind(user_id)
    .fetch_one(executor)
    .await?;
    Ok(row.0)
}

/// Allocates a change-log seq without writing the change-log row: for event
/// INSERT sites that record the seq as `events.insertion_seq` (insertion-order
/// tiebreak for same-time events, mirroring the app's `_id` order).
pub async fn alloc_seq<'e, E: sqlx::Executor<'e, Database = sqlx::Postgres>>(
    executor: E,
    user_id: i64,
) -> Result<i64, Problem> {
    next_seq(executor, user_id).await
}

/// Zone offsets are bounded by Java's `ZoneOffset` (±18h = ±1080 min);
/// `FixedOffset::east_opt` returns None outside that range, so the
/// export path would panic on legacy rows. Reject at every input path.
pub fn zone_offset_ok(offset_minutes: i64) -> bool {
    offset_minutes.unsigned_abs() <= 1080
}

/// Transactional variant: the change-log row lands in the caller's tx.
pub async fn append_change_tx(
    tx: &mut sqlx::PgConnection,
    user_id: i64,
    entity: &str,
    entity_id: &str,
    op: &str,
    payload: Value,
    origin: Option<Uuid>,
) -> Result<(), Problem> {
    let seq = next_seq(&mut *tx, user_id).await?;
    append_change_inner(
        &mut *tx, user_id, seq, entity, entity_id, op, payload, origin,
    )
    .await
}
#[allow(clippy::too_many_arguments)] // one row: fields mirror change_log columns
async fn append_change_inner(
    executor: impl sqlx::Executor<'_, Database = sqlx::Postgres>,
    user_id: i64,
    seq: i64,
    entity: &str,
    entity_id: &str,
    op: &str,
    payload: Value,
    origin: Option<Uuid>,
) -> Result<(), Problem> {
    sqlx::query(
        "INSERT INTO change_log (user_id, seq, entity, entity_id, op, payload, origin, occurred_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
    )
    .bind(user_id)
    .bind(seq)
    .bind(entity)
    .bind(entity_id)
    .bind(op)
    .bind(payload)
    .bind(origin)
    .execute(executor)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// date helpers (chrono 0.4.45 keeps num_days_from_ce crate-private)
// ---------------------------------------------------------------------------

/// Days since 1970-01-01 (Java `LocalDate.toEpochDay()`, matches the app).
pub fn date_to_epoch_day(d: chrono::NaiveDate) -> i32 {
    d.signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid"))
        .num_days() as i32
}

pub fn epoch_day_to_date(days: i32) -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap() + chrono::Duration::days(days as i64)
}

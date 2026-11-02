//! /events integration: list pagination is bounded like `/sync/changes`
//! (`limit` 1..=1000, `offset` >= 0) and the bounds do not disturb the
//! documented order/window semantics.

mod common;
use axum::http::StatusCode;
use common::*;

fn alice_token() -> String {
    forge_token("user-alice")
}

/// Bootstraps the user row (via `ensure_user`) and inserts `count` events with
/// strictly increasing `time`/`insertion_seq`, so expected pages are exact.
async fn seed_events(srv: &TestServer, token: &str, count: i64) {
    let _ = get(&srv.app, token, "/api/v1/me").await;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&srv.db_url)
        .await
        .expect("connect");
    let user: (i64,) = sqlx::query_as("SELECT id FROM users WHERE sub = $1")
        .bind("user-alice")
        .fetch_one(&pool)
        .await
        .expect("user");
    sqlx::query(
        "INSERT INTO events (id, user_id, time, zone_offset, type, updated_at, insertion_seq) \
         SELECT (md5(g::text || clock_timestamp()::text))::uuid, $1, 1700000000 + g, \
                0, 'CLOCK_IN', now(), g \
         FROM generate_series(1, $2) AS g",
    )
    .bind(user.0)
    .bind(count)
    .execute(&pool)
    .await
    .expect("seed events");
    pool.close().await;
}

fn len(body: &serde_json::Value) -> usize {
    body.as_array().expect("event array").len()
}

fn times(body: &serde_json::Value) -> Vec<i64> {
    body.as_array()
        .expect("event array")
        .iter()
        .map(|e| e["time"].as_i64().expect("time"))
        .collect()
}

#[tokio::test]
async fn list_events_clamps_limit_and_offset() {
    let srv = spawn_server().await;
    let alice = alice_token();
    seed_events(&srv, &alice, 1005).await;

    // Default page size (documented default: 500).
    let (status, body) = get(&srv.app, &alice, "/api/v1/events").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(len(&body), 500);

    // Above the documented max (1000) clamps to 1000 rather than asking
    // Postgres for a million rows.
    let (status, body) = get(&srv.app, &alice, "/api/v1/events?limit=1000000").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(len(&body), 1000);

    // Clamped limit still pages: the second page is exactly rows 1001..1005.
    let (status, body) = get(&srv.app, &alice, "/api/v1/events?limit=1000&offset=1000").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        times(&body),
        vec![1700001001, 1700001002, 1700001003, 1700001004, 1700001005]
    );

    // A negative offset must not reach Postgres (`OFFSET must not be
    // negative` would surface as a 500); it clamps to the first page.
    let (status, body) = get(&srv.app, &alice, "/api/v1/events?limit=2&offset=-9").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(times(&body), vec![1700000001, 1700000002]);

    // The `from` bound and the documented (time, insertion_seq) order survive
    // clamping: 6 rows remain from 1700001000 inclusive.
    let (status, body) = get(
        &srv.app,
        &alice,
        "/api/v1/events?from=1700001000&limit=1000000",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        times(&body),
        vec![1700001000, 1700001001, 1700001002, 1700001003, 1700001004, 1700001005]
    );

    common::drop_db(&srv.db_url).await;
}

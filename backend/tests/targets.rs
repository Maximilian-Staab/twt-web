//! /targets integration: listing is bounded like `/events`
//! (`limit` 1..=1000, `offset` >= 0) and the documented `(date, id)` order
//! survives clamping.

mod common;
use axum::http::StatusCode;
use common::*;
use serde_json::Value;

fn alice_token() -> String {
    forge_token("user-alice")
}

/// Bootstraps the user row (via `ensure_user`) and inserts `count` targets with
/// strictly increasing `time`, so expected pages are exact.
async fn seed_targets(srv: &TestServer, token: &str, count: i64) {
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
        "INSERT INTO targets (id, user_id, time, type, value, updated_at) \
         SELECT (md5(g::text || clock_timestamp()::text))::uuid, $1, \
                (19000 + g)::int, 'DAY_SET'::target_type, 480, now() \
         FROM generate_series(1, $2) AS g",
    )
    .bind(user.0)
    .bind(count)
    .execute(&pool)
    .await
    .expect("seed targets");
    pool.close().await;
}

fn dates(body: &Value) -> Vec<String> {
    body.as_array()
        .expect("target array")
        .iter()
        .map(|t| t["date"].as_str().expect("date").to_string())
        .collect()
}

#[tokio::test]
async fn list_targets_clamps_limit_and_offset() {
    let srv = spawn_server().await;
    let alice = alice_token();
    seed_targets(&srv, &alice, 1005).await;

    // Default page size (documented default: 500).
    let (status, default_page) = get(&srv.app, &alice, "/api/v1/targets").await;
    assert_eq!(status, StatusCode::OK, "body: {default_page}");
    let default_dates = dates(&default_page);
    assert_eq!(default_dates.len(), 500);

    // Above the documented max (1000) clamps to 1000 rather than asking
    // Postgres for a million rows.
    let (status, body) = get(&srv.app, &alice, "/api/v1/targets?limit=1000000").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(dates(&body).len(), 1000);

    // Clamped limit still pages: the second page is exactly rows 1001..1005 in
    // ascending date order.
    let (status, body) = get(&srv.app, &alice, "/api/v1/targets?limit=1000&offset=1000").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let page = dates(&body);
    assert_eq!(page.len(), 5);
    assert!(page[0] < page[4], "dates not ascending: {page:?}");

    // A negative offset must not reach Postgres (`OFFSET must not be
    // negative` would surface as a 500); it clamps to the first page.
    let (status, body) = get(&srv.app, &alice, "/api/v1/targets?limit=2&offset=-9").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(dates(&body), default_dates[..2].to_vec());

    common::drop_db(&srv.db_url).await;
}

//! /backup/events CSV round-trip: import → export → byte-stability.

mod common;

use common::*;
use serde_json::json;

fn alice_token() -> String {
    forge_token("user-alice")
}

/// CSV exactly as the app writes it (header + 2 event rows, one with task).
const APP_CSV: &str = "type;time;task;comment;taskId;name;active;ordering;isdefault\n\
                       in;2026-10-19T09:00:00+02:00;1;start;1;MyTask;1;0;1\n\
                       out;2026-10-19T17:30:00+02:00;1;done;1;;;;;\n";

#[tokio::test]
async fn csv_import_export_round_trip() {
    let srv = spawn_server().await;
    let alice = alice_token();

    // Import the app CSV.
    let (status, body) = put_raw_text(
        &srv.app,
        &alice,
        "/api/v1/backup/events",
        APP_CSV.to_string(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");
    let imported: serde_json::Value = serde_json::from_str(&body).expect("import result json");
    assert_eq!(imported["imported"], json!(2), "body: {body}");

    // Export: must succeed and be stable across re-imports of its own output.
    let (status, first) = get_raw(&srv.app, &alice, "/api/v1/backup/events").await;
    assert_eq!(status, axum::http::StatusCode::OK, "export failed: {first}");

    // Re-import the exported CSV; export again; must be byte-stable.
    let (status, _) = put_raw_text(&srv.app, &alice, "/api/v1/backup/events", first.clone()).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (_, second) = get_raw(&srv.app, &alice, "/api/v1/backup/events").await;
    assert_eq!(first, second, "export not stable across re-imports");

    common::drop_db(&srv.db_url).await;
}

/// A GET must not write: deriving task numbers used to persist `local_id`
/// (read-then-write, no transaction), so concurrent exports raced and the
/// export mutated task rows as a side effect.
#[tokio::test]
async fn export_is_read_only_and_deterministic() {
    let srv = spawn_server().await;
    let alice = alice_token();

    for name in ["beta", "alpha"] {
        let (status, body) = post(&srv.app, &alice, "/api/v1/tasks", json!({ "name": name })).await;
        assert_eq!(status, axum::http::StatusCode::CREATED, "create: {body}");
    }

    let (status, first) = get_raw(&srv.app, &alice, "/api/v1/backup/events").await;
    assert_eq!(status, axum::http::StatusCode::OK, "export failed: {first}");
    let (_, second) = get_raw(&srv.app, &alice, "/api/v1/backup/events").await;
    assert_eq!(first, second, "export is not deterministic");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&srv.db_url)
        .await
        .expect("connect");
    let assigned: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM tasks \
         WHERE user_id = (SELECT id FROM users WHERE sub = $1) AND local_id IS NOT NULL",
    )
    .bind("user-alice")
    .fetch_one(&pool)
    .await
    .expect("count local_id");
    pool.close().await;
    assert_eq!(assigned.0, 0, "backup GET persisted local_id");

    common::drop_db(&srv.db_url).await;
}

#[tokio::test]
async fn zone_offset_out_of_range_on_backup_export_clamps() {
    let srv = spawn_server().await;
    let alice = alice_token();

    // Bootstrap the user row (ensure_user) before the raw SQL insert.
    let _ = get(&srv.app, &alice, "/api/v1/me").await;

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
         VALUES ($1, $2, 1700000000, 99999, 'CLOCK_IN', now(), 1)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user.0)
    .execute(&pool)
    .await
    .expect("insert corrupt event");
    pool.close().await;

    // Export must not panic / kill the connection: expect 200 with clamp.
    let (status, body) = get_raw(&srv.app, &alice, "/api/v1/backup/events").await;
    assert_eq!(status, axum::http::StatusCode::OK, "export crashed: {body}");
    assert!(body.contains("+18:00"), "clamped offset missing: {body}");

    common::drop_db(&srv.db_url).await;
}

//! /sync integration: cross-tenant rejection, LWW, idempotency, clientId.

mod common;
use common::*;
use serde_json::json;
use tower::ServiceExt;

fn alice_token() -> String {
    forge_token("user-alice")
}
fn mallory_token() -> String {
    forge_token("user-mallory")
}

/// Registers a client for the given user; returns its clientId.
async fn register_client(srv: &TestServer, token: &str) -> String {
    let (status, body) = post(&srv.app, token, "/api/v1/clients", json!({"label": "test"})).await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "body: {body}");
    body["clientId"].as_str().unwrap().to_string()
}

/// Push helper: single op.
async fn push(
    srv: &TestServer,
    token: &str,
    client_id: &str,
    op: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    post(
        &srv.app,
        token,
        "/api/v1/sync/push",
        json!({ "clientId": client_id, "operations": [op] }),
    )
    .await
}

#[tokio::test]
async fn cross_tenant_push_is_rejected() {
    let srv = spawn_server().await;
    let alice = alice_token();
    let _alice_client = register_client(&srv, &alice).await;

    // Alice creates a task of her own.
    let (status, body) = post(
        &srv.app,
        &alice,
        "/api/v1/tasks",
        json!({"name": "alice-task", "active": true}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "body: {body}");
    let alice_task_id = body["id"].as_str().unwrap().to_string();

    // Mallory tries to overwrite alice's task via sync push.
    let mallory = mallory_token();
    let mallory_client = register_client(&srv, &mallory).await;
    let (status, body) = push(
        &srv,
        &mallory,
        &mallory_client,
        json!({
            "opId": "m-1",
            "entity": "task",
            "op": "upsert",
            "data": {
                "id": alice_task_id,
                "name": "HACKED",
                "active": true,
                "updatedAt": "2030-01-01T00:00:00Z",
            }
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["results"][0]["result"], "rejected", "body: {body}");
    assert_eq!(body["results"][0]["reason"], "id belongs to another user");

    // Alice's task must be untouched.
    let (status, body) = get(&srv.app, &alice, &format!("/api/v1/tasks/{alice_task_id}")).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        body["name"], "alice-task",
        "mallory overwrote alice's row: {body}"
    );

    common::drop_db(&srv.db_url).await;
}

#[tokio::test]
async fn push_requires_own_active_client() {
    let srv = spawn_server().await;
    let alice = alice_token();
    let alice_client = register_client(&srv, &alice).await;
    let mallory = mallory_token();

    // Mallory pushing with alice's clientId -> 403.
    let (status, body) = push(&srv, &mallory, &alice_client, json!({
        "opId": "m-2", "entity": "task", "op": "upsert",
        "data": {"id": "00000000-0000-0000-0000-000000000001", "name": "x", "active": true, "updatedAt": "2030-01-01T00:00:00Z"}
    })).await;
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN, "body: {body}");
    assert_eq!(
        body["detail"],
        "clientId is not an active client of the caller"
    );

    // Unknown clientId -> 403.
    let (status, _) = push(&srv, &alice, "11111111-1111-1111-1111-111111111111", json!({
        "opId": "a-1", "entity": "task", "op": "upsert",
        "data": {"id": "00000000-0000-0000-0000-000000000002", "name": "x", "active": true, "updatedAt": "2030-01-01T00:00:00Z"}
    })).await;
    assert_eq!(status, axum::http::StatusCode::FORBIDDEN);

    // Missing clientId -> 422 (axum Json rejection).
    let req = axum::http::Request::post("/api/v1/sync/push")
        .header("authorization", format!("Bearer {alice}"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            json!({"operations": []}).to_string(),
        ))
        .unwrap();
    let res = srv.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);

    common::drop_db(&srv.db_url).await;
}

#[tokio::test]
async fn lww_and_idempotency() {
    let srv = spawn_server().await;
    let alice = alice_token();
    let client = register_client(&srv, &alice).await;
    let event_id = "11111111-2222-3333-4444-555555555555";

    // Fresh write applies.
    let (status, body) = push(&srv, &alice, &client, json!({
        "opId": "op-1", "entity": "event", "op": "upsert",
        "data": {"id": event_id, "time": 1700000000, "zoneOffset": 60, "type": "CLOCK_IN", "updatedAt": "2030-01-01T00:00:00Z"}
    })).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(body["results"][0]["result"], "applied", "body: {body}");

    // Same opId replays as duplicate.
    let (_, body) = push(&srv, &alice, &client, json!({
        "opId": "op-1", "entity": "event", "op": "upsert",
        "data": {"id": event_id, "time": 1700000000, "zoneOffset": 60, "type": "CLOCK_IN", "updatedAt": "2030-01-01T00:00:00Z"}
    })).await;
    assert_eq!(body["results"][0]["result"], "duplicate", "body: {body}");

    // Staler updatedAt than stored -> ignoredStale.
    let (_, body) = push(&srv, &alice, &client, json!({
        "opId": "op-2", "entity": "event", "op": "upsert",
        "data": {"id": event_id, "time": 1700000000, "zoneOffset": 60, "type": "CLOCK_IN", "updatedAt": "2029-12-30T00:00:00Z"}
    })).await;
    assert_eq!(body["results"][0]["result"], "ignoredStale", "body: {body}");

    common::drop_db(&srv.db_url).await;
}

#[tokio::test]
async fn zone_offset_bounds_enforced() {
    let srv = spawn_server().await;
    let alice = alice_token();

    // REST create: 400 problem+json.
    let (status, body) = post(
        &srv.app,
        &alice,
        "/api/v1/events",
        json!({
            "time": 1700000000, "zoneOffset": 99999, "type": "CLOCK_IN"
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "body: {body}");

    // Sync push with out-of-range zoneOffset: op rejected.
    let client = register_client(&srv, &alice).await;
    let (_, body) = push(&srv, &alice, &client, json!({
        "opId": "zo-1", "entity": "event", "op": "upsert",
        "data": {"id": "22222222-3333-4444-5555-666666666666", "time": 1700000000, "zoneOffset": 99999, "type": "CLOCK_IN", "updatedAt": "2030-01-01T00:00:00Z"}
    })).await;
    assert_eq!(body["results"][0]["result"], "rejected", "body: {body}");
    assert_eq!(
        body["results"][0]["reason"],
        "zoneOffset out of range (±1080 min)"
    );

    common::drop_db(&srv.db_url).await;
}

/// PATCH must append a change-log payload the app can apply:
/// CloudSyncManager.applyTaskChange skips anything without name+active,
/// applyEventChange without time+type, applyTargetChange without date+type.
/// Regression: handlers used to append bare {"id"} / {"id","updatedAt"}.
#[tokio::test]
async fn patch_change_log_carries_full_row() {
    let srv = spawn_server().await;
    let alice = alice_token();

    // --- task ---
    let (status, task) = post(
        &srv.app,
        &alice,
        "/api/v1/tasks",
        json!({"name": "t1", "active": true}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "body: {task}");
    let task_id = task["id"].as_str().unwrap().to_string();
    let (status, _) = patch(
        &srv.app,
        &alice,
        &format!("/api/v1/tasks/{task_id}"),
        json!({"name": "renamed"}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (status, changes) = get(&srv.app, &alice, "/api/v1/sync/changes?cursor=0").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let task_changes: Vec<&serde_json::Value> = changes["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["entity"] == "task")
        .collect();
    let patch_change = task_changes.iter().next_back().unwrap();
    let data = &patch_change["data"];
    assert!(data.is_object(), "sparse task payload: {data}");
    assert_eq!(data["name"], "renamed", "payload: {data}");
    assert_eq!(data["active"], true, "payload: {data}");

    // --- event ---
    let (status, _) = post(
        &srv.app,
        &alice,
        "/api/v1/events",
        json!({
            "time": 1700000000, "zoneOffset": 60, "type": "CLOCK_IN"
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (_, events) = get(&srv.app, &alice, "/api/v1/events").await;
    let event_id = events[0]["id"].as_str().unwrap().to_string();
    let (status, _) = patch(
        &srv.app,
        &alice,
        &format!("/api/v1/events/{event_id}"),
        json!({
            "time": 1700000000, "zoneOffset": 60, "type": "CLOCK_IN", "comment": "patched"
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (_, changes) = get(&srv.app, &alice, "/api/v1/sync/changes?cursor=0").await;
    let event_change = changes["changes"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|c| c["entity"] == "event" && c["op"] == "upsert")
        .unwrap();
    let data = &event_change["data"];
    assert_eq!(data["time"], 1700000000, "payload: {data}");
    assert_eq!(data["type"], "CLOCK_IN", "payload: {data}");
    assert_eq!(data["comment"], "patched", "payload: {data}");

    // --- target ---
    let (status, target) = post(
        &srv.app,
        &alice,
        "/api/v1/targets",
        json!({
            "date": "2026-10-19", "type": "DAY_SET", "value": 480
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "body: {target}");
    let target_id = target["id"].as_str().unwrap().to_string();
    let (status, _) = patch(
        &srv.app,
        &alice,
        &format!("/api/v1/targets/{target_id}"),
        json!({"value": 240}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let (_, changes) = get(&srv.app, &alice, "/api/v1/sync/changes?cursor=0").await;
    let target_change = changes["changes"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|c| c["entity"] == "target" && c["op"] == "upsert")
        .unwrap();
    let data = &target_change["data"];
    assert_eq!(data["date"], "2026-10-19", "payload: {data}");
    assert_eq!(data["type"], "DAY_SET", "payload: {data}");
    assert_eq!(data["value"], 240, "payload: {data}");

    common::drop_db(&srv.db_url).await;
}

/// openapi EventUpsert.time declares minimum: 0; DESIGN §7.2 rejects
/// negative time on push. The REST paths must enforce the same bound.
#[tokio::test]
async fn negative_time_rejected_on_rest_paths() {
    let srv = spawn_server().await;
    let alice = alice_token();

    // Create rejects.
    let (status, body) = post(
        &srv.app,
        &alice,
        "/api/v1/events",
        json!({
            "time": -5, "zoneOffset": 60, "type": "CLOCK_IN"
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "body: {body}");

    // Create a valid event, then PATCH it to a negative time — rejected too.
    let (status, _) = post(
        &srv.app,
        &alice,
        "/api/v1/events",
        json!({
            "time": 1700000000, "zoneOffset": 60, "type": "CLOCK_IN"
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let (_, events) = get(&srv.app, &alice, "/api/v1/events").await;
    let event_id = events[0]["id"].as_str().unwrap().to_string();
    let (status, body) = patch(
        &srv.app,
        &alice,
        &format!("/api/v1/events/{event_id}"),
        json!({
            "time": -1, "zoneOffset": 60, "type": "CLOCK_IN"
        }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "body: {body}");

    // Snapshot import clamps negative time to 0 instead of dropping the row.
    let (status, body) = put(&srv.app, &alice, "/api/v1/import", json!({
        "events": [{"id": "33333333-4444-5555-6666-777777777777", "time": -300, "zoneOffset": 0, "type": "CLOCK_IN"}]
    })).await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");
    assert_eq!(body["imported"], 1, "body: {body}");
    let (status, ev) = get(
        &srv.app,
        &alice,
        "/api/v1/events/33333333-4444-5555-6666-777777777777",
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {ev}");
    assert_eq!(ev["time"], 0, "clamped time: {ev}");

    common::drop_db(&srv.db_url).await;
}

/// Regression: `PUT /import` used to count `ON CONFLICT DO NOTHING`-suppressed
/// writes as `imported`. Per the ImportResult contract (`openapi.yaml`) a
/// suppressed write is unchanged → `skipped`: own-row re-imports and
/// cross-tenant id collisions must not inflate `imported`, and the other
/// user's row must stay untouched.
#[tokio::test]
async fn import_counts_suppressed_writes_as_skipped() {
    let srv = spawn_server().await;
    let alice = alice_token();

    let snap = json!({
        "tasks": [{"id": "11111111-2222-3333-4444-555555555555", "name": "mine", "active": true}],
        "events": [{"id": "22222222-3333-4444-5555-666666666666",
                    "time": 1700000000, "zoneOffset": 60, "type": "CLOCK_IN"}],
        "targets": [{"id": "33333333-4444-5555-6666-777777777777",
                     "date": "2024-01-15", "type": "DAY_SET", "value": 480}],
        "preferences": {"theme": "dark"}
    });
    let (status, body) = put(&srv.app, &alice, "/api/v1/import", snap.clone()).await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");
    assert_eq!(
        body["imported"], 4,
        "first import must count all rows: {body}"
    );
    assert_eq!(body["skipped"], 0, "body: {body}");

    // Re-import: every row already exists → suppressed → skipped, not imported.
    let (status, body) = put(&srv.app, &alice, "/api/v1/import", snap).await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");
    assert_eq!(
        body["imported"], 0,
        "suppressed writes must not count as imported: {body}"
    );
    assert_eq!(body["skipped"], 4, "body: {body}");

    // Cross-tenant id collision: mallory reuses alice's task id. The insert is
    // suppressed (id exists, belongs to alice) and counted as skipped; alice's
    // row is untouched.
    let mallory = mallory_token();
    let (status, body) = put(&srv.app, &mallory, "/api/v1/import", json!({
        "tasks": [{"id": "11111111-2222-3333-4444-555555555555", "name": "hijack", "active": true}]
    })).await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");
    assert_eq!(
        body["imported"], 0,
        "cross-tenant suppression must not count as imported: {body}"
    );
    assert_eq!(body["skipped"], 1, "body: {body}");
    let (status, exported) = get(&srv.app, &alice, "/api/v1/export").await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {exported}");
    assert_eq!(
        exported["tasks"][0]["name"], "mine",
        "alice's task was modified: {exported}"
    );

    common::drop_db(&srv.db_url).await;
}

/// Every accepted push is client activity: `last_seen_at` must advance.
/// It backs `GET /clients` and the ordering of `GET /sync/state`.
#[tokio::test]
async fn push_refreshes_client_last_seen_at() {
    let srv = spawn_server().await;
    let alice = alice_token();
    let client = register_client(&srv, &alice).await;

    // Backdate the registration so the refresh is observable.
    let pool = sqlx::PgPool::connect(&srv.db_url).await.unwrap();
    sqlx::query("UPDATE clients SET last_seen_at = '2000-01-01T00:00:00Z' WHERE client_id = $1")
        .bind(uuid::Uuid::parse_str(&client).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let (status, body) = push(&srv, &alice, &client, json!({
        "opId": "seen-1", "entity": "task", "op": "upsert",
        "data": {"id": "44444444-5555-6666-7777-888888888888", "name": "seen", "active": true, "updatedAt": "2030-01-01T00:00:00Z"}
    })).await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");

    let (status, clients) = get(&srv.app, &alice, "/api/v1/clients").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let seen: chrono::DateTime<chrono::Utc> =
        clients[0]["lastSeenAt"].as_str().unwrap().parse().unwrap();
    assert!(
        seen > chrono::Utc::now() - chrono::TimeDelta::minutes(5),
        "lastSeenAt was not refreshed by push: {seen}"
    );

    common::drop_db(&srv.db_url).await;
}

/// Retention purge (openapi `/sync/changes` 410, DESIGN §7.1): expired
/// change-log rows are deleted and the per-user high-water mark recorded
/// (`change_retention.purged_through_seq`). A cursor inside the purged range
/// gets 410 problem+json; `cursor=0` and a cursor at the watermark still
/// page normally.
#[tokio::test]
async fn retention_purge_410s_stale_cursors() {
    let srv = spawn_server().await;
    let alice = alice_token();

    // Three task creates -> change_log seq 1..=3 for alice.
    for name in ["ret-a", "ret-b", "ret-c"] {
        let (status, body) = post(
            &srv.app,
            &alice,
            "/api/v1/tasks",
            json!({"name": name, "active": true}),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::CREATED, "body: {body}");
    }

    // Rows 1 and 2 are 400 days old: outside a 365-day window, row 3 is not.
    let pool = sqlx::PgPool::connect(&srv.db_url).await.unwrap();
    sqlx::query(
        "UPDATE change_log SET occurred_at = occurred_at - interval '400 days' \
         WHERE user_id = (SELECT id FROM users WHERE sub = 'user-alice') AND seq <= 2",
    )
    .execute(&pool)
    .await
    .unwrap();

    let purged = twt_cloud::routes::sync::purge_expired(&pool, 365)
        .await
        .unwrap();
    assert_eq!(purged, 2, "rows purged");

    // Watermark = highest purged seq.
    let (watermark,): (i64,) = sqlx::query_as("SELECT purged_through_seq FROM change_retention")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(watermark, 2);

    // Stale cursor inside the purged range -> 410 problem+json.
    let (status, body) = get(&srv.app, &alice, "/api/v1/sync/changes?cursor=1").await;
    assert_eq!(status, axum::http::StatusCode::GONE, "body: {body}");
    assert_eq!(body["type"], "cursor-expired", "body: {body}");
    assert_eq!(body["title"], "Sync cursor expired", "body: {body}");
    assert_eq!(body["status"], 410, "body: {body}");
    assert_eq!(
        body["detail"], "The requested cursor predates the retained change history.",
        "body: {body}"
    );

    // cursor=0 never 410s: replays the retained history (seq 3 only).
    let (status, body) = get(&srv.app, &alice, "/api/v1/sync/changes?cursor=0").await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");
    let changes = body["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "body: {body}");
    assert_eq!(changes[0]["seq"], 3, "body: {body}");
    assert_eq!(body["cursor"], 3, "body: {body}");

    // Cursor at the watermark: the client already saw everything that was
    // purged, so it pages normally instead of 410.
    let (status, body) = get(&srv.app, &alice, "/api/v1/sync/changes?cursor=2").await;
    assert_eq!(status, axum::http::StatusCode::OK, "body: {body}");
    let changes = body["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1, "body: {body}");
    assert_eq!(body["cursor"], 3, "body: {body}");

    pool.close().await;
    common::drop_db(&srv.db_url).await;
}

/// Tokens with a `none`/HS256 algorithm must be rejected even when the kid
/// matches a trusted key: `validate_token` pins the RS256/384/512 family
/// (`src/auth.rs`). `alg: none` strips the signature entirely; HS256 with
/// the public key as HMAC secret is the classic confusion attack.
#[tokio::test]
async fn non_rs256_tokens_are_rejected() {
    let srv = spawn_server().await;
    let exp = chrono::Utc::now().timestamp() + 3600;

    // `alg: none` — unsigned token carrying alice's claims and the trusted kid.
    let header = json!({"alg": "none", "typ": "JWT", "kid": "dev-key-1"});
    let claims = json!({
        "sub": "user-alice",
        "iss": "https://dev-idp.example.com",
        "aud": "twt-api",
        "exp": exp,
    });
    let b64 = |v: &serde_json::Value| {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string().as_bytes())
    };
    let token_none = format!("{}.{}.", b64(&header), b64(&claims));

    let (status, _) = get(&srv.app, &token_none, "/api/v1/me").await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    // HS256 with the RSA public key as HMAC secret (classic confusion).
    let key_pem = std::fs::read_to_string(common::dev_key_path()).unwrap();
    let token_hs = jsonwebtoken::encode(
        &jsonwebtoken::Header {
            alg: jsonwebtoken::Algorithm::HS256,
            kid: Some("dev-key-1".into()),
            ..Default::default()
        },
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(key_pem.as_bytes()),
    )
    .unwrap();
    let (status, _) = get(&srv.app, &token_hs, "/api/v1/me").await;
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);

    // Sanity: a real RS256 token still passes the same path.
    let (status, _) = get(&srv.app, &forge_token("user-alice"), "/api/v1/me").await;
    assert_eq!(status, axum::http::StatusCode::OK);

    common::drop_db(&srv.db_url).await;
}

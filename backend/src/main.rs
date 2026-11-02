//! Binary entrypoint: env setup, DB connect+migrate, bind, serve.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

use twt_cloud::api::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://twt:twt-dev@localhost:5432/twt_cloud".to_string());
    let bind_addr: SocketAddr = std::env::var("BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()
        .context("BIND_ADDR must be a valid socket address")?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&db_url)
        .await
        .context("connect to Postgres")?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let auth = twt_cloud::auth::AuthConfig::from_env().await?;
    let state = AppState { pool, auth };

    // Change-log retention (openapi /sync/changes 410, DESIGN §7.1): purge
    // expired rows hourly in the background. The first interval tick fires
    // immediately, so the purge also runs once at startup.
    let retention_days = std::env::var("RETENTION_DAYS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(180)
        .max(0);
    let purge = {
        let pool = state.pool.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3600));
            loop {
                tick.tick().await;
                match twt_cloud::routes::sync::purge_expired(&pool, retention_days).await {
                    Ok(n) if n > 0 => tracing::info!("purged {n} expired change-log rows"),
                    Ok(_) => {}
                    Err(e) => tracing::error!("change-log purge failed: {e}"),
                }
            }
        })
    };

    // Keep a pool handle for the shutdown drain; PgPool clones share one pool.
    let pool = state.pool.clone();
    let app = twt_cloud::api::router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!("listening on {bind_addr}");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;

    // Drain finished: cancel the background purge and close the pool.
    purge.abort();
    pool.close().await;
    result?;
    Ok(())
}

/// Resolves on the first stop signal: ctrl-c on any platform, SIGTERM on unix.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install ctrl-c handler")
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

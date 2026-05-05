mod checker;
mod config;
mod db;
mod routes;
mod state;
mod telegram;

use anyhow::{Context, Result};
use axum::routing::{get, post};
use axum::Router;
use dashmap::DashMap;
use sqlx::postgres::PgPoolOptions;
use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tower_http::trace::TraceLayer;

#[tokio::main]
async fn main() -> Result<()> {
    let app_config = config::load_env_config();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,status_board=info".into()),
        )
        .init();

    let services = config::load_services(&app_config.services_config_path)?;
    tracing::info!("loaded {} services from {}", services.len(), app_config.services_config_path);

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&app_config.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;

    db::init(&pool).await?;
    tracing::info!("database initialized");

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .user_agent("status-board/1.0")
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .context("failed to build HTTP client")?;

    let state = Arc::new(AppState {
        services,
        states: DashMap::new(),
        db: pool,
        http_client,
        config: app_config.clone(),
    });

    tokio::spawn(checker::spawn_checker(state.clone()));

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            match db::cleanup_old_checks(&cleanup_state.db, 90).await {
                Ok(n) if n > 0 => tracing::info!("cleaned up {n} old check records"),
                Err(e) => tracing::error!("cleanup failed: {e}"),
                _ => {}
            }
        }
    });

    let mut app = Router::new()
        .route("/", get(routes::index))
        .route("/health", get(routes::health))
        .route("/api/status", get(routes::api_status))
        .route("/api/status/:id", get(routes::api_status_detail))
        .route("/api/history", get(routes::api_history))
        .route("/api/incidents", get(routes::api_incidents));

    if app_config.admin_token.is_some() {
        app = app.route("/api/reload", post(routes::reload_config));
    }

    let app = app
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("0.0.0.0:{}", app_config.port);
    tracing::info!("listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("failed to bind")?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => tracing::info!("received ctrl+c, shutting down"),
            _ = sigterm.recv() => tracing::info!("received SIGTERM, shutting down"),
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        tracing::info!("received ctrl+c, shutting down");
    }
}

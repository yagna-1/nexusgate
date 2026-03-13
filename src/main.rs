mod api;
mod auth;
mod budget;
mod config;
mod error;
mod models;
mod providers;
mod rate_limit;
mod router;
mod state;

use std::sync::Arc;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present (dev convenience)
    dotenvy::dotenv().ok();

    // Structured logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nexusgate=info,tower_http=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // ── Configuration ──────────────────────────────────────────────────────
    let cfg = config::Config::from_env()?;

    let configured = cfg.configured_providers();
    if configured.is_empty() {
        tracing::warn!(
            "No LLM provider API keys set. Set OPENAI_API_KEY, ANTHROPIC_API_KEY, or GEMINI_API_KEY."
        );
    } else {
        info!(providers = ?configured, "Configured providers");
    }

    // ── Redis ──────────────────────────────────────────────────────────────
    let redis_client = redis::Client::open(cfg.redis_url.as_str())
        .map_err(|e| anyhow::anyhow!("Redis connection failed: {e}"))?;

    let redis_manager = redis::aio::ConnectionManager::new(redis_client)
        .await
        .map_err(|e| anyhow::anyhow!("Redis manager init failed: {e}"))?;

    info!("Redis connected");

    // ── SQLite ─────────────────────────────────────────────────────────────
    // Ensure data directory exists
    if let Some(path) = cfg.database_url.strip_prefix("sqlite:") {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
    }

    let db = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(16)
        .connect_with(
            cfg.database_url
                .parse::<sqlx::sqlite::SqliteConnectOptions>()?
                .create_if_missing(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                .synchronous(sqlx::sqlite::SqliteSynchronous::Normal),
        )
        .await
        .map_err(|e| anyhow::anyhow!("SQLite connection failed: {e}"))?;

    info!("SQLite connected");

    // ── Migrations ─────────────────────────────────────────────────────────
    run_migrations(&db).await?;

    // ── App State ──────────────────────────────────────────────────────────
    let state = Arc::new(state::AppState::new(cfg.clone(), db, redis_manager));

    // ── First-run: print admin token ───────────────────────────────────────
    let admin_token = auth::issue_admin_token(&cfg.admin_jwt_secret)?;
    info!("═══════════════════════════════════════════════════");
    info!("Admin JWT token (valid 1 year):");
    info!("{}", admin_token);
    info!("Dashboard: http://{}:{}", cfg.host, cfg.port);
    info!("═══════════════════════════════════════════════════");

    // ── Server ─────────────────────────────────────────────────────────────
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind {addr}: {e}"))?;

    info!("🚀 NexusGate listening on http://{}", addr);

    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn run_migrations(db: &sqlx::SqlitePool) -> anyhow::Result<()> {
    let sql = include_str!("../migrations/001_init.sql");
    for statement in sql.split(';') {
        let without_comments = statement
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = without_comments.trim();
        if !trimmed.is_empty() {
            sqlx::query(trimmed).execute(db).await?;
        }
    }
    info!("Migrations applied");
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install Ctrl+C handler");
    info!("Shutdown signal received");
}

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceConfig {
    pub id: String,
    pub name: String,
    pub url: String,
    pub category: String,
    pub check: String,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub port: u16,
    pub db_path: String,
    pub check_interval_secs: u64,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub admin_token: Option<String>,
    pub services_config_path: String,
    pub site_title: String,
    pub site_url: String,
    pub footer_text: String,
}

pub fn load_env_config() -> AppConfig {
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse()
        .unwrap_or(3000);

    let telegram_bot_token = std::env::var("TELEGRAM_BOT_TOKEN").ok().filter(|s| !s.is_empty());
    let telegram_chat_id = std::env::var("TELEGRAM_CHAT_ID").ok().filter(|s| !s.is_empty());
    let admin_token = std::env::var("ADMIN_TOKEN").ok().filter(|s| !s.is_empty());

    AppConfig {
        port,
        db_path: std::env::var("DB_PATH").unwrap_or_else(|_| "data/status.db".into()),
        check_interval_secs: std::env::var("CHECK_INTERVAL_SECS")
            .unwrap_or_else(|_| "60".into())
            .parse()
            .unwrap_or(60),
        telegram_bot_token,
        telegram_chat_id,
        admin_token,
        services_config_path: std::env::var("SERVICES_CONFIG")
            .unwrap_or_else(|_| "services.json".into()),
        site_title: std::env::var("SITE_TITLE")
            .unwrap_or_else(|_| "Status Board".into()),
        site_url: std::env::var("SITE_URL").unwrap_or_default(),
        footer_text: std::env::var("FOOTER_TEXT")
            .unwrap_or_else(|_| "Powered by Status Board · Rust/Axum".into()),
    }
}

pub fn load_services(path: &str) -> Result<Vec<ServiceConfig>> {
    let content = std::fs::read_to_string(Path::new(path))
        .with_context(|| format!("failed to read services config: {path}"))?;
    let services: Vec<ServiceConfig> =
        serde_json::from_str(&content).context("failed to parse services config")?;
    Ok(services)
}

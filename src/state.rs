use crate::config::{AppConfig, ServiceConfig};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Serialize;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Up,
    Degraded,
    Down,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Up => write!(f, "up"),
            Status::Degraded => write!(f, "degraded"),
            Status::Down => write!(f, "down"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ServiceState {
    pub status: Status,
    pub latency_ms: u32,
    pub last_check: DateTime<Utc>,
    pub last_change: DateTime<Utc>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
}

pub struct AppState {
    pub services: Vec<ServiceConfig>,
    pub states: DashMap<String, ServiceState>,
    pub db: PgPool,
    pub http_client: reqwest::Client,
    pub config: AppConfig,
}

pub type SharedState = Arc<AppState>;

use crate::db;
use crate::state::{SharedState, Status};
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use serde::Serialize;
use std::collections::HashMap;

const INDEX_HTML: &str = include_str!("../templates/index.html");

pub async fn index(State(state): State<SharedState>) -> impl IntoResponse {
    let html = INDEX_HTML
        .replace("{{SITE_TITLE}}", &state.config.site_title)
        .replace("{{SITE_URL}}", &state.config.site_url)
        .replace("{{FOOTER_TEXT}}", &state.config.footer_text)
        .replace(
            "{{CHECK_INTERVAL}}",
            &state.config.check_interval_secs.to_string(),
        );

    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html)
}

pub async fn health() -> &'static str {
    "ok"
}

pub async fn api_status(
    State(state): State<SharedState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let conn = state.db.lock().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("db lock: {e}"))
    })?;

    let mut services = Vec::new();
    let mut up_count = 0u32;
    let total = state.services.len() as u32;

    for svc in &state.services {
        let svc_state = state.states.get(&svc.id);

        let (status, latency_ms, last_check, last_error) = match &svc_state {
            Some(s) => (s.status, s.latency_ms, Some(s.last_check.to_rfc3339()), s.last_error.clone()),
            None => (Status::Up, 0, None, None),
        };

        if status == Status::Up || status == Status::Degraded {
            up_count += 1;
        }

        let uptime = db::get_uptime_pct(&conn, &svc.id).unwrap_or(None);

        let last_incident = db::get_service_incidents(&conn, &svc.id, 1)
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|i| i.started_at);

        services.push(ServiceStatusJson {
            id: svc.id.clone(),
            name: svc.name.clone(),
            url: svc.url.clone(),
            category: svc.category.clone(),
            status,
            latency_ms,
            uptime_30d_pct: uptime,
            last_check,
            last_incident,
            last_error,
        });
    }

    let has_degraded = services.iter().any(|s| s.status == Status::Degraded);
    let has_down = services.iter().any(|s| s.status == Status::Down);

    let global = if has_down {
        if (up_count as f64 / total as f64) <= 0.5 {
            "major_outage"
        } else {
            "partial_outage"
        }
    } else if has_degraded {
        "partial_degradation"
    } else {
        "operational"
    };

    let body = StatusResponse {
        global: global.to_string(),
        checked_at: chrono::Utc::now().to_rfc3339(),
        up_count,
        total,
        services,
    };

    Ok(axum::Json(body))
}

pub async fn api_status_detail(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let svc = state
        .services
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("service not found: {id}")))?;

    let svc_state = state.states.get(&id);
    let (status, latency_ms) = match &svc_state {
        Some(s) => (s.status, s.latency_ms),
        None => (Status::Up, 0),
    };

    let conn = state.db.lock().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("db lock: {e}"))
    })?;

    let uptime = db::get_uptime_pct(&conn, &id).unwrap_or(None);

    let latency_24h = db::get_latency_history(&conn, &id, 1440)
        .unwrap_or_default()
        .into_iter()
        .map(|(time, ms)| LatencyPoint { time, ms })
        .collect();

    let recent_incidents = db::get_service_incidents(&conn, &id, 10)
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            let duration_secs = match (&i.resolved_at, &i.started_at) {
                (Some(resolved), started) => {
                    chrono::DateTime::parse_from_rfc3339(resolved)
                        .ok()
                        .and_then(|r| {
                            chrono::DateTime::parse_from_rfc3339(started)
                                .ok()
                                .map(|s| (r - s).num_seconds())
                        })
                }
                _ => None,
            };
            IncidentJson {
                started_at: i.started_at,
                resolved_at: i.resolved_at,
                duration_secs,
                kind: i.kind,
                error: i.error,
            }
        })
        .collect();

    let body = ServiceDetailJson {
        id: svc.id.clone(),
        name: svc.name.clone(),
        url: svc.url.clone(),
        status,
        latency_ms,
        uptime_30d_pct: uptime,
        latency_24h,
        recent_incidents,
    };

    Ok(axum::Json(body))
}

pub async fn api_history(
    State(state): State<SharedState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let conn = state.db.lock().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("db lock: {e}"))
    })?;

    let mut history = Vec::new();
    for svc in &state.services {
        let daily = db::get_daily_uptime(&conn, &svc.id, 90).unwrap_or_default();
        let days: Vec<DayUptime> = daily
            .into_iter()
            .map(|(date, pct)| DayUptime {
                date: date.to_string(),
                pct,
            })
            .collect();

        history.push(ServiceHistory {
            id: svc.id.clone(),
            name: svc.name.clone(),
            days,
        });
    }

    Ok(axum::Json(history))
}

pub async fn api_incidents(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let limit: u32 = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let conn = state.db.lock().map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("db lock: {e}"))
    })?;

    let incidents = db::get_incidents(&conn, limit).unwrap_or_default();

    let name_map: HashMap<&str, &str> = state
        .services
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_str()))
        .collect();

    let body: Vec<IncidentListJson> = incidents
        .into_iter()
        .map(|i| {
            let duration_secs = match (&i.resolved_at, &i.started_at) {
                (Some(resolved), started) => {
                    chrono::DateTime::parse_from_rfc3339(resolved)
                        .ok()
                        .and_then(|r| {
                            chrono::DateTime::parse_from_rfc3339(started)
                                .ok()
                                .map(|s| (r - s).num_seconds())
                        })
                }
                _ => None,
            };
            IncidentListJson {
                service_id: i.service_id.clone(),
                service_name: name_map
                    .get(i.service_id.as_str())
                    .unwrap_or(&"unknown")
                    .to_string(),
                kind: i.kind,
                started_at: i.started_at,
                resolved_at: i.resolved_at,
                duration_secs,
                error: i.error,
            }
        })
        .collect();

    Ok(axum::Json(body))
}

pub async fn reload_config(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let token = state
        .config
        .admin_token
        .as_ref()
        .ok_or((StatusCode::NOT_FOUND, "not found".to_string()))?;

    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let expected = format!("Bearer {token}");
    if auth != expected {
        return Err((StatusCode::UNAUTHORIZED, "unauthorized".to_string()));
    }

    let services = crate::config::load_services(&state.config.services_config_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("reload failed: {e}")))?;

    tracing::info!("reloaded {} services from config", services.len());
    Ok(axum::Json(serde_json::json!({
        "reloaded": services.len()
    })))
}

#[derive(Serialize)]
struct StatusResponse {
    global: String,
    checked_at: String,
    up_count: u32,
    total: u32,
    services: Vec<ServiceStatusJson>,
}

#[derive(Serialize)]
struct ServiceStatusJson {
    id: String,
    name: String,
    url: String,
    category: String,
    status: Status,
    latency_ms: u32,
    uptime_30d_pct: Option<f64>,
    last_check: Option<String>,
    last_incident: Option<String>,
    last_error: Option<String>,
}

#[derive(Serialize)]
struct ServiceDetailJson {
    id: String,
    name: String,
    url: String,
    status: Status,
    latency_ms: u32,
    uptime_30d_pct: Option<f64>,
    latency_24h: Vec<LatencyPoint>,
    recent_incidents: Vec<IncidentJson>,
}

#[derive(Serialize)]
struct LatencyPoint {
    time: String,
    ms: Option<u32>,
}

#[derive(Serialize)]
struct IncidentJson {
    started_at: String,
    resolved_at: Option<String>,
    duration_secs: Option<i64>,
    kind: String,
    error: Option<String>,
}

#[derive(Serialize)]
struct ServiceHistory {
    id: String,
    name: String,
    days: Vec<DayUptime>,
}

#[derive(Serialize)]
struct DayUptime {
    date: String,
    pct: f64,
}

#[derive(Serialize)]
struct IncidentListJson {
    service_id: String,
    service_name: String,
    kind: String,
    started_at: String,
    resolved_at: Option<String>,
    duration_secs: Option<i64>,
    error: Option<String>,
}

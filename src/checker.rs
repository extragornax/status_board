use crate::config::ServiceConfig;
use crate::state::{SharedState, ServiceState, Status};
use crate::{db, telegram};
use chrono::Utc;
use std::time::Duration;
use tokio::task::JoinSet;

pub async fn spawn_checker(state: SharedState) {
    let interval = Duration::from_secs(state.config.check_interval_secs);

    loop {
        run_checks(&state).await;
        tokio::time::sleep(interval).await;
    }
}

async fn run_checks(state: &SharedState) {
    let mut set = JoinSet::new();
    let client = state.http_client.clone();

    for service in &state.services {
        let client = client.clone();
        let service = service.clone();
        set.spawn(async move {
            let start = std::time::Instant::now();
            let result = client.get(&service.url).send().await;
            let elapsed = start.elapsed();
            (service, result, elapsed)
        });
    }

    while let Some(res) = set.join_next().await {
        match res {
            Ok((service, result, elapsed)) => {
                process_check(state, &service, result, elapsed).await;
            }
            Err(e) => tracing::error!("check task panicked: {e}"),
        }
    }

    tracing::debug!("completed check cycle for {} services", state.services.len());
}

async fn process_check(
    state: &SharedState,
    service: &ServiceConfig,
    result: Result<reqwest::Response, reqwest::Error>,
    elapsed: Duration,
) {
    let latency_ms = elapsed.as_millis() as u32;
    let now = Utc::now();

    let (new_status, error_msg, http_code, got_response) = match &result {
        Ok(resp) => {
            let code = resp.status().as_u16();
            if resp.status().is_success() {
                if latency_ms > 3000 {
                    (Status::Degraded, None, Some(code as i32), true)
                } else {
                    (Status::Up, None, Some(code as i32), true)
                }
            } else {
                (Status::Down, Some(format!("HTTP {code}")), Some(code as i32), true)
            }
        }
        Err(e) => {
            let msg = if e.is_timeout() {
                "connection timeout".to_string()
            } else if e.is_connect() {
                "connection refused".to_string()
            } else {
                e.to_string()
            };
            (Status::Down, Some(msg), None, false)
        }
    };

    let _ = db::insert_check(
        &state.db,
        &service.id,
        &new_status.to_string(),
        if got_response { Some(latency_ms as i32) } else { None },
        error_msg.as_deref(),
        http_code,
    )
    .await;

    let previous = state.states.get(&service.id).map(|s| s.clone());
    let (prev_status, prev_consecutive_failures, prev_last_change) = match &previous {
        Some(s) => (Some(s.status), s.consecutive_failures, s.last_change),
        None => (None, 0, now),
    };

    let consecutive_failures = if new_status == Status::Down {
        prev_consecutive_failures + 1
    } else {
        0
    };

    let effective_status = if new_status == Status::Down && consecutive_failures < 3 {
        prev_status.unwrap_or(Status::Up)
    } else {
        new_status
    };

    let status_changed = prev_status.is_some() && prev_status != Some(effective_status);
    let last_change = if status_changed { now } else { prev_last_change };

    state.states.insert(
        service.id.clone(),
        ServiceState {
            status: effective_status,
            latency_ms: if got_response { latency_ms } else { 0 },
            last_check: now,
            last_change,
            consecutive_failures,
            last_error: error_msg.clone(),
        },
    );

    if !status_changed {
        return;
    }

    match effective_status {
        Status::Down => {
            let _ = db::open_incident(&state.db, &service.id, "down", error_msg.as_deref()).await;
        }
        Status::Degraded => {
            let _ = db::close_incident(&state.db, &service.id).await;
            let _ = db::open_incident(&state.db, &service.id, "degraded", None).await;
        }
        Status::Up => {
            let _ = db::close_incident(&state.db, &service.id).await;
        }
    }

    let (token, chat_id) = match (&state.config.telegram_bot_token, &state.config.telegram_chat_id) {
        (Some(t), Some(c)) => (t.clone(), c.clone()),
        _ => return,
    };

    let message = match effective_status {
        Status::Down => telegram::format_down(
            &service.name,
            &service.url,
            error_msg.as_deref().unwrap_or("unknown"),
        ),
        Status::Up => {
            let downtime = (now - prev_last_change).num_seconds();
            telegram::format_up(&service.name, &service.url, downtime)
        }
        Status::Degraded => telegram::format_degraded(&service.name, &service.url, latency_ms),
    };

    let client = state.http_client.clone();
    tokio::spawn(async move {
        if let Err(e) = telegram::send_alert(&client, &token, &chat_id, &message).await {
            tracing::error!("telegram alert failed: {e}");
        }
    });
}

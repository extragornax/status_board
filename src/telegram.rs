use anyhow::{Context, Result};
use serde_json::json;

pub async fn send_alert(
    client: &reqwest::Client,
    token: &str,
    chat_id: &str,
    message: &str,
) -> Result<()> {
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");

    client
        .post(&url)
        .json(&json!({
            "chat_id": chat_id,
            "text": message,
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        }))
        .send()
        .await
        .context("failed to send Telegram alert")?;

    Ok(())
}

pub fn format_down(name: &str, url: &str, error: &str) -> String {
    let time = chrono::Utc::now().format("%H:%M UTC");
    format!("\u{1f534} <b>DOWN \u{2014} {name}</b>\n{url}\nSince: {time}\nError: {error}")
}

pub fn format_up(name: &str, url: &str, downtime_secs: i64) -> String {
    let time = chrono::Utc::now().format("%H:%M UTC");
    let duration = format_duration(downtime_secs);
    format!("\u{1f7e2} <b>UP \u{2014} {name}</b>\n{url}\nRecovered at: {time}\nDowntime: {duration}")
}

pub fn format_degraded(name: &str, url: &str, latency_ms: u32) -> String {
    format!("\u{1f7e1} <b>SLOW \u{2014} {name}</b>\n{url}\nLatency: {latency_ms} ms (threshold: 3000 ms)")
}

fn format_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}min", secs / 60)
    } else {
        format!("{}h {}min", secs / 3600, (secs % 3600) / 60)
    }
}

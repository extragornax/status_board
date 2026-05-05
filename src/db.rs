use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use rusqlite::Connection;
use std::path::Path;

pub fn init(path: &str) -> Result<Connection> {
    if let Some(parent) = Path::new(path).parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create db directory: {}", parent.display()))?;
    }

    let conn = Connection::open(path).context("failed to open SQLite database")?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         PRAGMA foreign_keys=ON;",
    )
    .context("failed to set SQLite pragmas")?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS checks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            service_id  TEXT    NOT NULL,
            checked_at  TEXT    NOT NULL,
            status      TEXT    NOT NULL,
            latency_ms  INTEGER,
            error       TEXT,
            http_code   INTEGER
        );

        CREATE TABLE IF NOT EXISTS incidents (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            service_id  TEXT    NOT NULL,
            type        TEXT    NOT NULL,
            started_at  TEXT    NOT NULL,
            resolved_at TEXT,
            error       TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_checks_service_time ON checks(service_id, checked_at);
        CREATE INDEX IF NOT EXISTS idx_incidents_service ON incidents(service_id, started_at);",
    )
    .context("failed to create schema")?;

    Ok(conn)
}

pub fn insert_check(
    conn: &Connection,
    service_id: &str,
    status: &str,
    latency_ms: Option<u32>,
    error: Option<&str>,
    http_code: Option<u16>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO checks (service_id, checked_at, status, latency_ms, error, http_code)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![service_id, now, status, latency_ms, error, http_code],
    )
    .context("failed to insert check")?;
    Ok(())
}

pub fn open_incident(conn: &Connection, service_id: &str, kind: &str, error: Option<&str>) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO incidents (service_id, type, started_at, error)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![service_id, kind, now, error],
    )
    .context("failed to open incident")?;
    Ok(())
}

pub fn close_incident(conn: &Connection, service_id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE incidents SET resolved_at = ?1
         WHERE service_id = ?2 AND resolved_at IS NULL",
        rusqlite::params![now, service_id],
    )
    .context("failed to close incident")?;
    Ok(())
}

pub fn get_uptime_pct(conn: &Connection, service_id: &str) -> Result<Option<f64>> {
    let mut stmt = conn.prepare(
        "SELECT
            SUM(CASE WHEN status = 'up' OR status = 'degraded' THEN 1 ELSE 0 END) AS up_count,
            COUNT(*) AS total_count
         FROM checks
         WHERE service_id = ?1
           AND checked_at > datetime('now', '-30 days')",
    )?;

    let result = stmt.query_row(rusqlite::params![service_id], |row| {
        let up: Option<i64> = row.get(0)?;
        let total: i64 = row.get(1)?;
        Ok((up.unwrap_or(0), total))
    })?;

    if result.1 == 0 {
        Ok(None)
    } else {
        Ok(Some(result.0 as f64 * 100.0 / result.1 as f64))
    }
}

pub fn get_latency_history(conn: &Connection, service_id: &str, limit: u32) -> Result<Vec<(String, Option<u32>)>> {
    let mut stmt = conn.prepare(
        "SELECT checked_at, latency_ms FROM checks
         WHERE service_id = ?1
         ORDER BY checked_at DESC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(rusqlite::params![service_id, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<u32>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_incidents(conn: &Connection, limit: u32) -> Result<Vec<IncidentRow>> {
    let mut stmt = conn.prepare(
        "SELECT service_id, type, started_at, resolved_at, error
         FROM incidents
         ORDER BY started_at DESC
         LIMIT ?1",
    )?;

    let rows = stmt
        .query_map(rusqlite::params![limit], |row| {
            Ok(IncidentRow {
                service_id: row.get(0)?,
                kind: row.get(1)?,
                started_at: row.get(2)?,
                resolved_at: row.get(3)?,
                error: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

pub fn get_service_incidents(conn: &Connection, service_id: &str, limit: u32) -> Result<Vec<IncidentRow>> {
    let mut stmt = conn.prepare(
        "SELECT service_id, type, started_at, resolved_at, error
         FROM incidents
         WHERE service_id = ?1
         ORDER BY started_at DESC
         LIMIT ?2",
    )?;

    let rows = stmt
        .query_map(rusqlite::params![service_id, limit], |row| {
            Ok(IncidentRow {
                service_id: row.get(0)?,
                kind: row.get(1)?,
                started_at: row.get(2)?,
                resolved_at: row.get(3)?,
                error: row.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    Ok(rows)
}

#[derive(Debug)]
pub struct IncidentRow {
    pub service_id: String,
    pub kind: String,
    pub started_at: String,
    pub resolved_at: Option<String>,
    pub error: Option<String>,
}

pub fn get_daily_uptime(conn: &Connection, service_id: &str, days: u32) -> Result<Vec<(NaiveDate, f64)>> {
    let mut stmt = conn.prepare(
        "SELECT date(checked_at) as day,
                SUM(CASE WHEN status = 'up' OR status = 'degraded' THEN 1 ELSE 0 END) * 100.0 / COUNT(*) as pct
         FROM checks
         WHERE service_id = ?1
           AND checked_at > datetime('now', ?2)
         GROUP BY day
         ORDER BY day ASC",
    )?;

    let offset = format!("-{days} days");
    let rows = stmt
        .query_map(rusqlite::params![service_id, offset], |row| {
            let day_str: String = row.get(0)?;
            let pct: f64 = row.get(1)?;
            Ok((day_str, pct))
        })?
        .filter_map(|r| {
            r.ok().and_then(|(day_str, pct)| {
                NaiveDate::parse_from_str(&day_str, "%Y-%m-%d")
                    .ok()
                    .map(|d| (d, pct))
            })
        })
        .collect();

    Ok(rows)
}

pub fn cleanup_old_checks(conn: &Connection, days: u32) -> Result<usize> {
    let offset = format!("-{days} days");
    let deleted = conn.execute(
        "DELETE FROM checks WHERE checked_at < datetime('now', ?1)",
        rusqlite::params![offset],
    )?;
    Ok(deleted)
}

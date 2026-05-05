use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::postgres::PgPool;
use sqlx::Row;

pub async fn init(pool: &PgPool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS checks (
            id          BIGSERIAL PRIMARY KEY,
            service_id  TEXT    NOT NULL,
            checked_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            status      TEXT    NOT NULL,
            latency_ms  INTEGER,
            error       TEXT,
            http_code   INTEGER
        )",
    )
    .execute(pool)
    .await
    .context("failed to create checks table")?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS incidents (
            id          BIGSERIAL PRIMARY KEY,
            service_id  TEXT    NOT NULL,
            type        TEXT    NOT NULL,
            started_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            resolved_at TIMESTAMPTZ,
            error       TEXT
        )",
    )
    .execute(pool)
    .await
    .context("failed to create incidents table")?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_checks_service_time ON checks(service_id, checked_at)")
        .execute(pool)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_incidents_service ON incidents(service_id, started_at)")
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn insert_check(
    pool: &PgPool,
    service_id: &str,
    status: &str,
    latency_ms: Option<i32>,
    error: Option<&str>,
    http_code: Option<i32>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO checks (service_id, status, latency_ms, error, http_code)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(service_id)
    .bind(status)
    .bind(latency_ms)
    .bind(error)
    .bind(http_code)
    .execute(pool)
    .await
    .context("failed to insert check")?;
    Ok(())
}

pub async fn open_incident(pool: &PgPool, service_id: &str, kind: &str, error: Option<&str>) -> Result<()> {
    sqlx::query(
        "INSERT INTO incidents (service_id, type, error) VALUES ($1, $2, $3)",
    )
    .bind(service_id)
    .bind(kind)
    .bind(error)
    .execute(pool)
    .await
    .context("failed to open incident")?;
    Ok(())
}

pub async fn close_incident(pool: &PgPool, service_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE incidents SET resolved_at = NOW()
         WHERE service_id = $1 AND resolved_at IS NULL",
    )
    .bind(service_id)
    .execute(pool)
    .await
    .context("failed to close incident")?;
    Ok(())
}

pub async fn get_uptime_pct(pool: &PgPool, service_id: &str) -> Result<Option<f64>> {
    let row = sqlx::query(
        "SELECT
            COALESCE(SUM(CASE WHEN status = 'up' OR status = 'degraded' THEN 1 ELSE 0 END), 0) AS up_count,
            COUNT(*) AS total_count
         FROM checks
         WHERE service_id = $1
           AND checked_at > NOW() - INTERVAL '30 days'",
    )
    .bind(service_id)
    .fetch_one(pool)
    .await?;

    let up: i64 = row.get("up_count");
    let total: i64 = row.get("total_count");

    if total == 0 {
        Ok(None)
    } else {
        Ok(Some(up as f64 * 100.0 / total as f64))
    }
}

pub async fn get_latency_history(
    pool: &PgPool,
    service_id: &str,
    limit: i64,
) -> Result<Vec<(DateTime<Utc>, Option<i32>)>> {
    let rows = sqlx::query(
        "SELECT checked_at, latency_ms FROM checks
         WHERE service_id = $1
         ORDER BY checked_at DESC
         LIMIT $2",
    )
    .bind(service_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| (r.get("checked_at"), r.get("latency_ms")))
        .collect())
}

pub async fn get_incidents(pool: &PgPool, limit: i64) -> Result<Vec<IncidentRow>> {
    let rows = sqlx::query(
        "SELECT service_id, type, started_at, resolved_at, error
         FROM incidents
         ORDER BY started_at DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| IncidentRow {
            service_id: r.get("service_id"),
            kind: r.get("type"),
            started_at: r.get("started_at"),
            resolved_at: r.get("resolved_at"),
            error: r.get("error"),
        })
        .collect())
}

pub async fn get_service_incidents(
    pool: &PgPool,
    service_id: &str,
    limit: i64,
) -> Result<Vec<IncidentRow>> {
    let rows = sqlx::query(
        "SELECT service_id, type, started_at, resolved_at, error
         FROM incidents
         WHERE service_id = $1
         ORDER BY started_at DESC
         LIMIT $2",
    )
    .bind(service_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| IncidentRow {
            service_id: r.get("service_id"),
            kind: r.get("type"),
            started_at: r.get("started_at"),
            resolved_at: r.get("resolved_at"),
            error: r.get("error"),
        })
        .collect())
}

#[derive(Debug)]
pub struct IncidentRow {
    pub service_id: String,
    pub kind: String,
    pub started_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

pub async fn get_daily_uptime(pool: &PgPool, service_id: &str, days: i32) -> Result<Vec<(NaiveDate, f64)>> {
    let rows = sqlx::query(
        "SELECT checked_at::date as day,
                COALESCE(SUM(CASE WHEN status = 'up' OR status = 'degraded' THEN 1 ELSE 0 END), 0) * 100.0 / COUNT(*) as pct
         FROM checks
         WHERE service_id = $1
           AND checked_at > NOW() - make_interval(days => $2)
         GROUP BY day
         ORDER BY day ASC",
    )
    .bind(service_id)
    .bind(days)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| {
            let day: NaiveDate = r.get("day");
            let pct: f64 = r.get("pct");
            (day, pct)
        })
        .collect())
}

pub async fn cleanup_old_checks(pool: &PgPool, days: i32) -> Result<u64> {
    let result = sqlx::query(
        "DELETE FROM checks WHERE checked_at < NOW() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

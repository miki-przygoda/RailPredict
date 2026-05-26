//! Static site export — writes a self-contained HTML snapshot of the last N days'
//! delay history and prediction accuracy to a single file.
//!
//! Invoked via the `export-site` CLI subcommand.  The output is a standalone
//! HTML page with Chart.js (CDN) and all data baked in as a JSON literal — no
//! server or database connection needed to view it.

use std::path::Path;

use chrono::Utc;
use serde::Serialize;
use sqlx::FromRow;

use crate::db::Db;

const TEMPLATE: &str = include_str!("template.html");

// ---------------------------------------------------------------------------
// Output data types (serialised into the page as JSON)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ExportSummary {
    pub total_observations: i64,
    pub total_services: i64,
    pub mean_actual_mins: Option<f64>,
    pub mean_abs_error_mins: Option<f64>,
    pub prediction_coverage_pct: Option<f64>,
    pub on_time_pct: Option<f64>,
}

#[derive(Serialize)]
pub struct DailyStat {
    pub day: String,
    pub observations: i64,
    pub mean_actual_mins: Option<f64>,
    pub mean_abs_error_mins: Option<f64>,
    pub on_time_pct: Option<f64>,
}

#[derive(Serialize)]
pub struct ServiceStat {
    pub uid: String,
    pub origin_crs: String,
    pub weekday: i16,
    pub departure_hour: i16,
    pub observations: i64,
    pub mean_actual: Option<f64>,
    pub mean_predicted: Option<f64>,
    pub on_time_pct: Option<f64>,
    pub coverage_pct: Option<f64>,
}

#[derive(Serialize)]
pub struct HourlyStat {
    pub hour: i16,
    pub observations: i64,
    pub mean_delay: Option<f64>,
}

#[derive(Serialize)]
pub struct ExportData {
    pub generated_at: String,
    pub days_window: u32,
    pub summary: ExportSummary,
    pub daily: Vec<DailyStat>,
    pub services: Vec<ServiceStat>,
    pub hourly: Vec<HourlyStat>,
}

// ---------------------------------------------------------------------------
// DB row types (not serialised — internal only)
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct SummaryRow {
    total_observations: i64,
    total_services: i64,
    mean_actual_mins: Option<f64>,
    mean_abs_error_mins: Option<f64>,
    prediction_coverage_pct: Option<f64>,
    on_time_pct: Option<f64>,
}

#[derive(FromRow)]
struct DailyRow {
    day: String,
    observations: i64,
    mean_actual_mins: Option<f64>,
    mean_abs_error_mins: Option<f64>,
    on_time_pct: Option<f64>,
}

#[derive(FromRow)]
struct ServiceRow {
    uid: String,
    origin_crs: String,
    weekday: i16,
    departure_hour: i16,
    observations: i64,
    mean_actual: Option<f64>,
    mean_predicted: Option<f64>,
    on_time_pct: Option<f64>,
    coverage_pct: Option<f64>,
}

#[derive(FromRow)]
struct HourlyRow {
    hour: i16,
    observations: i64,
    mean_delay: Option<f64>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Query the database, render the HTML template, and write to `output_path`.
/// `days` controls the look-back window (default 7).
pub async fn export_site(db: &Db, output_path: &Path, days: u32) -> anyhow::Result<()> {
    let data = gather(db, days).await?;

    let total_obs = data.summary.total_observations;
    let total_svcs = data.summary.total_services;
    // Escape `</` so a stray `</script>` in a string field can't terminate the
    // host <script> tag. `\/` is a valid JSON escape for `/`, so the parsed
    // values are unchanged.
    let json = serde_json::to_string(&data)?.replace("</", "<\\/");

    let html = TEMPLATE.replace("__EXPORT_DATA__", &json);

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, html)?;

    tracing::info!(
        path = %output_path.display(),
        observations = total_obs,
        services = total_svcs,
        days,
        "Static site exported"
    );
    println!(
        "Exported {} observations across {} services → {}",
        total_obs,
        total_svcs,
        output_path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Data gathering
// ---------------------------------------------------------------------------

async fn gather(db: &Db, days: u32) -> anyhow::Result<ExportData> {
    let days_i = days as i32;
    let generated_at = Utc::now().format("%d %b %Y %H:%M UTC").to_string();

    let (summary, daily, services, hourly) = tokio::try_join!(
        query_summary(db, days_i),
        query_daily(db, days_i),
        query_services(db, days_i),
        query_hourly(db, days_i),
    )?;

    Ok(ExportData { generated_at, days_window: days, summary, daily, services, hourly })
}

async fn query_summary(db: &Db, days: i32) -> anyhow::Result<ExportSummary> {
    let row = sqlx::query_as::<_, SummaryRow>(
        r#"
        SELECT
            COUNT(*)                                                             AS total_observations,
            COUNT(DISTINCT uid || '|' || origin_crs)                            AS total_services,
            AVG(delay_mins::FLOAT8)                                             AS mean_actual_mins,
            AVG(ABS(delay_mins::FLOAT8 - predicted_delay_mins::FLOAT8))
                FILTER (WHERE predicted_delay_mins IS NOT NULL)                 AS mean_abs_error_mins,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * COUNT(predicted_delay_mins)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                  AS prediction_coverage_pct,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * SUM(CASE WHEN delay_mins <= 0 THEN 1 ELSE 0 END)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                  AS on_time_pct
        FROM delay_history
        WHERE recorded_at >= NOW() - $1::INT * INTERVAL '1 day'
        "#,
    )
    .bind(days)
    .fetch_one(db)
    .await?;

    Ok(ExportSummary {
        total_observations:     row.total_observations,
        total_services:         row.total_services,
        mean_actual_mins:       row.mean_actual_mins,
        mean_abs_error_mins:    row.mean_abs_error_mins,
        prediction_coverage_pct: row.prediction_coverage_pct,
        on_time_pct:            row.on_time_pct,
    })
}

async fn query_daily(db: &Db, days: i32) -> anyhow::Result<Vec<DailyStat>> {
    let rows = sqlx::query_as::<_, DailyRow>(
        r#"
        SELECT
            TO_CHAR(recorded_at::DATE, 'YYYY-MM-DD')                           AS day,
            COUNT(*)                                                            AS observations,
            AVG(delay_mins::FLOAT8)                                            AS mean_actual_mins,
            AVG(ABS(delay_mins::FLOAT8 - predicted_delay_mins::FLOAT8))
                FILTER (WHERE predicted_delay_mins IS NOT NULL)                AS mean_abs_error_mins,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * SUM(CASE WHEN delay_mins <= 0 THEN 1 ELSE 0 END)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                 AS on_time_pct
        FROM delay_history
        WHERE recorded_at >= NOW() - $1::INT * INTERVAL '1 day'
        GROUP BY recorded_at::DATE
        ORDER BY recorded_at::DATE ASC
        "#,
    )
    .bind(days)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(|r| DailyStat {
        day:              r.day,
        observations:     r.observations,
        mean_actual_mins: r.mean_actual_mins,
        mean_abs_error_mins: r.mean_abs_error_mins,
        on_time_pct:      r.on_time_pct,
    }).collect())
}

async fn query_services(db: &Db, days: i32) -> anyhow::Result<Vec<ServiceStat>> {
    let rows = sqlx::query_as::<_, ServiceRow>(
        r#"
        SELECT
            uid,
            origin_crs,
            weekday,
            departure_hour,
            COUNT(*)                                                             AS observations,
            AVG(delay_mins::FLOAT8)                                             AS mean_actual,
            AVG(predicted_delay_mins::FLOAT8)
                FILTER (WHERE predicted_delay_mins IS NOT NULL)                 AS mean_predicted,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * SUM(CASE WHEN delay_mins <= 0 THEN 1 ELSE 0 END)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                  AS on_time_pct,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * COUNT(predicted_delay_mins)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                  AS coverage_pct
        FROM delay_history
        WHERE recorded_at >= NOW() - $1::INT * INTERVAL '1 day'
        GROUP BY uid, origin_crs, weekday, departure_hour
        ORDER BY observations DESC
        LIMIT 100
        "#,
    )
    .bind(days)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(|r| ServiceStat {
        uid:            r.uid.trim().to_string(),
        origin_crs:     r.origin_crs.trim().to_string(),
        weekday:        r.weekday,
        departure_hour: r.departure_hour,
        observations:   r.observations,
        mean_actual:    r.mean_actual,
        mean_predicted: r.mean_predicted,
        on_time_pct:    r.on_time_pct,
        coverage_pct:   r.coverage_pct,
    }).collect())
}

async fn query_hourly(db: &Db, days: i32) -> anyhow::Result<Vec<HourlyStat>> {
    let rows = sqlx::query_as::<_, HourlyRow>(
        r#"
        SELECT
            departure_hour                AS hour,
            COUNT(*)                      AS observations,
            AVG(delay_mins::FLOAT8)       AS mean_delay
        FROM delay_history
        WHERE recorded_at >= NOW() - $1::INT * INTERVAL '1 day'
        GROUP BY departure_hour
        ORDER BY departure_hour
        "#,
    )
    .bind(days)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(|r| HourlyStat {
        hour:         r.hour,
        observations: r.observations,
        mean_delay:   r.mean_delay,
    }).collect())
}

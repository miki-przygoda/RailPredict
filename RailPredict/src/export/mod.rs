//! Static site export — writes a self-contained HTML snapshot of the last N days'
//! delay history and prediction accuracy to a single file.
//!
//! Invoked via the `export-site` CLI subcommand.  The output is a standalone
//! HTML page with Chart.js (CDN) and all data baked in as a JSON literal — no
//! server or database connection needed to view it.

use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
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
    pub mean_predicted_mins: Option<f64>,
    pub mean_abs_error_mins: Option<f64>,
    pub prediction_coverage_pct: Option<f64>,
    pub on_time_pct: Option<f64>,
    /// % of predictions within ±5 min of actual delay (among predicted rows only).
    pub pct_within_5min: Option<f64>,
    /// % of predictions within ±10 min of actual delay (among predicted rows only).
    pub pct_within_10min: Option<f64>,
}

#[derive(Serialize)]
pub struct DailyStat {
    pub day: String,
    pub observations: i64,
    pub mean_actual_mins: Option<f64>,
    pub mean_predicted_mins: Option<f64>,
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

#[derive(Serialize, Deserialize, Clone)]
pub struct ModelBenchmarkStats {
    pub mae:          f64,
    pub rmse:         f64,
    pub bias:         f64,
    pub within_2min:  f64,
    pub within_5min:  f64,
    pub within_10min: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ModelBenchmarks {
    pub trained_at:   String,
    pub train_rows:   u64,
    pub test_rows:    u64,
    pub baseline_mae: f64,
    pub day_ahead:    ModelBenchmarkStats,
    pub realtime:     ModelBenchmarkStats,
}

#[derive(Serialize)]
pub struct ExportData {
    pub generated_at: String,
    pub days_window:  u32,
    pub summary:      ExportSummary,
    pub daily:        Vec<DailyStat>,
    pub services:     Vec<ServiceStat>,
    pub hourly:       Vec<HourlyStat>,
    /// Training test-set benchmarks from the last `compare_models.py` run.
    /// `None` if `models/benchmarks.json` doesn't exist yet.
    pub benchmarks:   Option<ModelBenchmarks>,
}

// ---------------------------------------------------------------------------
// DB row types (not serialised — internal only)
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct SummaryRow {
    total_observations: i64,
    total_services: i64,
    mean_actual_mins: Option<f64>,
    mean_predicted_mins: Option<f64>,
    mean_abs_error_mins: Option<f64>,
    prediction_coverage_pct: Option<f64>,
    on_time_pct: Option<f64>,
    pct_within_5min: Option<f64>,
    pct_within_10min: Option<f64>,
}

#[derive(FromRow)]
struct DailyRow {
    day: String,
    observations: i64,
    mean_actual_mins: Option<f64>,
    mean_predicted_mins: Option<f64>,
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

/// Render the HTML report for `days` days of history and return it as a string.
pub async fn render_html(db: &Db, days: u32) -> anyhow::Result<String> {
    let data = gather(db, days).await?;
    // Escape `</` so a stray `</script>` in a string field can't terminate the
    // host <script> tag. `\/` is a valid JSON escape for `/`, so the parsed
    // values are unchanged.
    let json = serde_json::to_string(&data)?.replace("</", "<\\/");
    Ok(TEMPLATE.replace("__EXPORT_DATA__", &json))
}

/// Query the database, render the HTML template, and write to `output_path`.
/// `days` controls the look-back window (default 7).
pub async fn export_site(db: &Db, output_path: &Path, days: u32) -> anyhow::Result<()> {
    let data = gather(db, days).await?;
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
        observations = data.summary.total_observations,
        services = data.summary.total_services,
        days,
        "Static site exported"
    );
    println!(
        "Exported {} observations across {} services → {}",
        data.summary.total_observations,
        data.summary.total_services,
        output_path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Data gathering
// ---------------------------------------------------------------------------

fn load_benchmarks() -> Option<ModelBenchmarks> {
    // Walk up from the binary's working directory looking for models/benchmarks.json.
    let candidates = [
        Path::new("models/benchmarks.json").to_path_buf(),
        Path::new("../models/benchmarks.json").to_path_buf(),
    ];
    for path in &candidates {
        if let Ok(text) = std::fs::read_to_string(path)
            && let Ok(b) = serde_json::from_str::<ModelBenchmarks>(&text)
        {
            return Some(b);
        }
    }
    None
}

async fn gather(db: &Db, days: u32) -> anyhow::Result<ExportData> {
    let days_i = days as i32;
    let generated_at = Utc::now().format("%d %b %Y %H:%M UTC").to_string();
    let benchmarks = load_benchmarks();

    let (summary, daily, services, hourly) = tokio::try_join!(
        query_summary(db, days_i),
        query_daily(db, days_i),
        query_services(db, days_i),
        query_hourly(db, days_i),
    )?;

    Ok(ExportData { generated_at, days_window: days, summary, daily, services, hourly, benchmarks })
}

async fn query_summary(db: &Db, days: i32) -> anyhow::Result<ExportSummary> {
    let row = sqlx::query_as::<_, SummaryRow>(
        r#"
        SELECT
            COUNT(*)                                                             AS total_observations,
            COUNT(DISTINCT uid || '|' || origin_crs)                            AS total_services,
            AVG(delay_mins::FLOAT8)                                             AS mean_actual_mins,
            AVG(predicted_delay_mins::FLOAT8)
                FILTER (WHERE predicted_delay_mins IS NOT NULL)                 AS mean_predicted_mins,
            AVG(ABS(delay_mins::FLOAT8 - predicted_delay_mins::FLOAT8))
                FILTER (WHERE predicted_delay_mins IS NOT NULL)                 AS mean_abs_error_mins,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * COUNT(predicted_delay_mins)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                  AS prediction_coverage_pct,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * SUM(CASE WHEN delay_mins <= 0 THEN 1 ELSE 0 END)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                  AS on_time_pct,
            100.0 * SUM(CASE WHEN ABS(delay_mins - predicted_delay_mins) <= 5
                                  AND predicted_delay_mins IS NOT NULL THEN 1 ELSE 0 END)::FLOAT8
                / NULLIF(COUNT(predicted_delay_mins), 0)                        AS pct_within_5min,
            100.0 * SUM(CASE WHEN ABS(delay_mins - predicted_delay_mins) <= 10
                                  AND predicted_delay_mins IS NOT NULL THEN 1 ELSE 0 END)::FLOAT8
                / NULLIF(COUNT(predicted_delay_mins), 0)                        AS pct_within_10min
        FROM delay_history
        WHERE recorded_at >= NOW() - $1::INT * INTERVAL '1 day'
          AND delay_mins BETWEEN -120 AND 600
        "#,
    )
    .bind(days)
    .fetch_one(db)
    .await?;

    Ok(ExportSummary {
        total_observations:      row.total_observations,
        total_services:          row.total_services,
        mean_actual_mins:        row.mean_actual_mins,
        mean_predicted_mins:     row.mean_predicted_mins,
        mean_abs_error_mins:     row.mean_abs_error_mins,
        prediction_coverage_pct: row.prediction_coverage_pct,
        on_time_pct:             row.on_time_pct,
        pct_within_5min:         row.pct_within_5min,
        pct_within_10min:        row.pct_within_10min,
    })
}

async fn query_daily(db: &Db, days: i32) -> anyhow::Result<Vec<DailyStat>> {
    let rows = sqlx::query_as::<_, DailyRow>(
        r#"
        SELECT
            TO_CHAR(recorded_at::DATE, 'YYYY-MM-DD')                           AS day,
            COUNT(*)                                                            AS observations,
            AVG(delay_mins::FLOAT8)                                            AS mean_actual_mins,
            AVG(predicted_delay_mins::FLOAT8)
                FILTER (WHERE predicted_delay_mins IS NOT NULL)                AS mean_predicted_mins,
            AVG(ABS(delay_mins::FLOAT8 - predicted_delay_mins::FLOAT8))
                FILTER (WHERE predicted_delay_mins IS NOT NULL)                AS mean_abs_error_mins,
            CASE WHEN COUNT(*) > 0
                 THEN 100.0 * SUM(CASE WHEN delay_mins <= 0 THEN 1 ELSE 0 END)::FLOAT8 / COUNT(*)
                 ELSE NULL END                                                 AS on_time_pct
        FROM delay_history
        WHERE recorded_at >= NOW() - $1::INT * INTERVAL '1 day'
          AND delay_mins BETWEEN -120 AND 600
        GROUP BY recorded_at::DATE
        ORDER BY recorded_at::DATE ASC
        "#,
    )
    .bind(days)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(|r| DailyStat {
        day:                 r.day,
        observations:        r.observations,
        mean_actual_mins:    r.mean_actual_mins,
        mean_predicted_mins: r.mean_predicted_mins,
        mean_abs_error_mins: r.mean_abs_error_mins,
        on_time_pct:         r.on_time_pct,
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
          AND delay_mins BETWEEN -120 AND 600
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
          AND delay_mins BETWEEN -120 AND 600
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

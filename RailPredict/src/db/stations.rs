//! Read-only per-station / per-route analytics for the `/stations/:crs` explorer.
//!
//! Aggregates `delay_history` filtered to a single `origin_crs`, joining
//! `services` (on `uid`) where route O–D pairs are needed. All queries apply
//! the standard delay sanity filter (`delay_mins BETWEEN -120 AND 600`).
//!
//! Both `StationSummary` and the per-service `ServiceRow` carry `std_delay_mins`
//! (`STDDEV_SAMP` of delay) — the spread that drives the Reliable/Variable
//! reliability score on the explorer, not just the mean.

use super::Db;

/// Reliability headline for departures from a single station over a rolling window.
///
/// A pure aggregate (no `GROUP BY`) always returns exactly one row, so a
/// `sample_count` of 0 signals "no observations in window" rather than a missing row.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct StationSummary {
    /// The station CRS this summary covers (echoed from the `$1` bind).
    pub crs: String,
    /// Percentage of departures that ran on time (delay ≤ 0). `None` when empty.
    pub on_time_pct: Option<f64>,
    /// Mean recorded delay in minutes. `None` when empty.
    pub avg_delay_mins: Option<f64>,
    /// Std-dev of delay (minutes) — the stochastic component. Low = "reliably ~X late",
    /// high = "wildly variable". `None` with fewer than 2 samples.
    pub std_delay_mins: Option<f64>,
    /// Mean absolute prediction error in minutes over rows that carried a prediction. `None` when none.
    pub mae_mins: Option<f64>,
    /// Number of observations in the window (0 = no data).
    pub sample_count: i64,
}

/// One non-empty `(weekday, departure_hour)` cell of the delay heatmap.
///
/// The caller pivots these into a dense 7×24 grid; only populated cells are returned.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct HeatCell {
    /// Day of week, 0 = Monday .. 6 = Sunday.
    pub weekday: i16,
    /// Scheduled departure hour, 0..23.
    pub departure_hour: i16,
    /// Mean recorded delay in minutes for this cell.
    pub avg_delay_mins: Option<f64>,
    /// On-time percentage for this cell.
    pub on_time_pct: Option<f64>,
    /// Number of observations in this cell.
    pub sample_count: i64,
}

/// A service departing this station, ranked by observation count.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct ServiceRow {
    /// RTTI UID of the recurring service.
    pub uid: String,
    /// Destination CRS from the `services` join. `None` for UIDs absent from the timetable (LEFT JOIN).
    pub destination_crs: Option<String>,
    /// Operating company (GTFS agency_id) from the `services` join. `None` if unmapped.
    pub toc: Option<String>,
    /// Number of observations for this service in the window.
    pub sample_count: i64,
    /// Mean recorded delay in minutes for this service.
    pub avg_delay_mins: Option<f64>,
    /// Std-dev of delay (minutes) — this service's reliability (low = dependable). `None` < 2 obs.
    pub std_delay_mins: Option<f64>,
    /// On-time percentage for this service.
    pub on_time_pct: Option<f64>,
}

/// One origin station in the "busiest stations" index, by observation count.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct OriginRow {
    /// The origin location code as stored in `delay_history` (TIPLOC-style).
    pub code: String,
    pub sample_count: i64,
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
}

/// Busiest origin stations by observation count over the last `hours` hours.
///
/// NOTE (perf): a full `GROUP BY origin_crs` over `delay_history` (~7M rows).
/// The `/stations` index is not a hot path; if it becomes one, back this with a
/// periodically-refreshed summary table.
pub async fn busiest_origins(db: &Db, hours: i32, limit: i64) -> sqlx::Result<Vec<OriginRow>> {
    sqlx::query_as::<_, OriginRow>(
        r#"
        SELECT
            d.origin_crs                                                          AS code,
            COUNT(*)                                                              AS sample_count,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            AVG(d.delay_mins::float8)                                             AS avg_delay_mins
        FROM delay_history d
        WHERE d.origin_crs IS NOT NULL
          AND d.recorded_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        GROUP BY d.origin_crs
        ORDER BY sample_count DESC
        LIMIT $2
        "#,
    )
    .bind(hours)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Reliability headline for departures from `crs` over the last `hours` hours.
///
/// The aggregate always produces one row; `sample_count == 0` means the window
/// held no observations. Returns `Ok(Some(_))` in the normal case.
pub async fn station_summary(
    db: &Db,
    crs: &str,
    hours: i32,
) -> sqlx::Result<Option<StationSummary>> {
    sqlx::query_as::<_, StationSummary>(
        r#"
        SELECT
            $1                                                                    AS crs,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            AVG(d.delay_mins::float8)                                             AS avg_delay_mins,
            STDDEV_SAMP(d.delay_mins::float8)                                     AS std_delay_mins,
            (AVG(ABS(d.predicted_delay_mins - d.delay_mins))
                FILTER (WHERE d.predicted_delay_mins IS NOT NULL))::float8         AS mae_mins,
            COUNT(*)                                                              AS sample_count
        FROM delay_history d
        WHERE d.origin_crs = $1
          AND d.recorded_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        "#,
    )
    .bind(crs)
    .bind(hours)
    .fetch_optional(db)
    .await
}

/// Delay heatmap cells keyed by `(weekday, departure_hour)` for `crs` over `hours`.
///
/// Returns only non-empty cells, ordered by weekday then hour.
pub async fn station_heatmap(db: &Db, crs: &str, hours: i32) -> sqlx::Result<Vec<HeatCell>> {
    sqlx::query_as::<_, HeatCell>(
        r#"
        SELECT
            d.weekday                                                             AS weekday,
            d.departure_hour                                                      AS departure_hour,
            AVG(d.delay_mins::float8)                                             AS avg_delay_mins,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            COUNT(*)                                                              AS sample_count
        FROM delay_history d
        WHERE d.origin_crs = $1
          AND d.recorded_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        GROUP BY d.weekday, d.departure_hour
        ORDER BY d.weekday, d.departure_hour
        "#,
    )
    .bind(crs)
    .bind(hours)
    .fetch_all(db)
    .await
}

/// Top `limit` services departing `crs` by observation count over `hours`.
///
/// LEFT JOINs `services` so UIDs absent from the timetable still appear (with
/// `destination_crs`/`toc` as `None`).
pub async fn station_busiest_services(
    db: &Db,
    crs: &str,
    hours: i32,
    limit: i64,
) -> sqlx::Result<Vec<ServiceRow>> {
    sqlx::query_as::<_, ServiceRow>(
        r#"
        SELECT
            d.uid                                                                 AS uid,
            s.destination_crs                                                     AS destination_crs,
            s.toc                                                                 AS toc,
            COUNT(*)                                                              AS sample_count,
            AVG(d.delay_mins::float8)                                             AS avg_delay_mins,
            STDDEV_SAMP(d.delay_mins::float8)                                     AS std_delay_mins,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct
        FROM delay_history d
        LEFT JOIN services s ON s.uid = d.uid
        WHERE d.origin_crs = $1
          AND d.recorded_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        GROUP BY d.uid, s.destination_crs, s.toc
        ORDER BY sample_count DESC
        LIMIT $3
        "#,
    )
    .bind(crs)
    .bind(hours)
    .bind(limit)
    .fetch_all(db)
    .await
}

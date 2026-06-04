//! Read access to the operators reference table.

use crate::db::Db;

/// An operator (TOC) reference row: code, friendly name, brand colour.
#[derive(Debug, Clone, sqlx::FromRow, PartialEq, Eq)]
pub struct Operator {
    pub toc: String,
    pub name: String,
    pub brand_color: String,
}

/// List all operators, ordered by friendly name.
pub async fn list_operators(db: &Db) -> sqlx::Result<Vec<Operator>> {
    sqlx::query_as::<_, Operator>(
        "SELECT toc, name, brand_color FROM operators ORDER BY name",
    )
    .fetch_all(db)
    .await
}

/// One operator's punctuality aggregates over a rolling window — a league row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OperatorLeagueRow {
    pub toc: String,
    pub name: String,
    pub brand_color: String,
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub sample_count: i64,
}

/// Operator league table over the last `hours` hours: delay_history JOINed to
/// services (by uid) and operators (by toc), grouped by operator, ranked by
/// on-time %. Only operators with at least `min_samples` rows are included.
/// Returns empty until `services.toc` is populated (run the GTFS ingest).
pub async fn operator_league(
    db: &Db,
    hours: i32,
    min_samples: i64,
    limit: i64,
) -> sqlx::Result<Vec<OperatorLeagueRow>> {
    sqlx::query_as::<_, OperatorLeagueRow>(
        r#"
        SELECT
            s.toc                                                               AS toc,
            COALESCE(o.name, s.toc)                                             AS name,
            COALESCE(o.brand_color, '#9aa7b4')                                  AS brand_color,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            AVG(d.delay_mins::float8)                                           AS avg_delay_mins,
            COUNT(*)                                                            AS sample_count
        FROM delay_history d
        JOIN services s ON s.uid = d.uid
        LEFT JOIN operators o ON o.toc = s.toc
        WHERE d.recorded_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
          AND s.toc IS NOT NULL
        GROUP BY s.toc, o.name, o.brand_color
        HAVING COUNT(*) >= $2
        ORDER BY on_time_pct DESC NULLS LAST
        LIMIT $3
        "#,
    )
    .bind(hours)
    .bind(min_samples)
    .bind(limit)
    .fetch_all(db)
    .await
}

// ---------------------------------------------------------------------------
// Per-operator drill-down (the `/operators/:toc` page).
//
// All four queries below filter on a single operator (`s.toc = $1`), window on
// `d.recorded_at` over the last `hours` hours, and apply the same sanity bound
// (`d.delay_mins BETWEEN -120 AND 600`) as the league table. Identity is reached
// by JOINing `delay_history d` → `services s` (on `uid`); route O–D pairs come
// from `services` (delay_history has no destination column). These are pure
// reads — they never write to `delay_history`.
// ---------------------------------------------------------------------------

/// Headline aggregates for one operator over a rolling window.
///
/// `on_time_pct` is the share of observations with `delay_mins <= 0`; `mae_mins`
/// is the mean absolute prediction error over rows that captured a prediction.
/// `name`/`brand_color` fall back to the TOC code and a neutral grey when the
/// operator is absent from the `operators` reference table.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct OperatorDetail {
    /// TOC code (`services.toc`).
    pub toc: String,
    /// Friendly name, or the TOC code when no `operators` row exists.
    pub name: String,
    /// Brand colour `#rrggbb`, or `#9aa7b4` when unknown.
    pub brand_color: String,
    /// Percentage of observations on time (`delay_mins <= 0`), 0–100.
    pub on_time_pct: Option<f64>,
    /// Mean delay in minutes over the window.
    pub avg_delay_mins: Option<f64>,
    /// Mean absolute prediction error (minutes) over rows with a captured prediction.
    pub mae_mins: Option<f64>,
    /// Number of observations in the window.
    pub sample_count: i64,
}

/// Single-operator headline aggregates over the last `hours` hours.
///
/// Returns `None` when the operator has no in-window observations. Joins
/// `delay_history` → `services` (by uid) and LEFT JOINs `operators` (by toc),
/// so name/colour gracefully fall back when the reference row is missing.
pub async fn operator_detail(
    db: &Db,
    toc: &str,
    hours: i32,
) -> sqlx::Result<Option<OperatorDetail>> {
    sqlx::query_as::<_, OperatorDetail>(
        r#"
        SELECT
            s.toc                                                                  AS toc,
            COALESCE(o.name, s.toc)                                                AS name,
            COALESCE(o.brand_color, '#9aa7b4')                                     AS brand_color,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            AVG(d.delay_mins::float8)                                              AS avg_delay_mins,
            (AVG(ABS(d.predicted_delay_mins - d.delay_mins))
                FILTER (WHERE d.predicted_delay_mins IS NOT NULL))::float8         AS mae_mins,
            COUNT(*)                                                               AS sample_count
        FROM delay_history d
        JOIN services s ON s.uid = d.uid
        LEFT JOIN operators o ON o.toc = s.toc
        WHERE s.toc = $1
          AND d.recorded_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        GROUP BY s.toc, o.name, o.brand_color
        "#,
    )
    .bind(toc)
    .bind(hours)
    .fetch_optional(db)
    .await
}

/// One day's punctuality aggregates for an operator — a point on the trend line.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct OperatorDailyPoint {
    /// Calendar day (Europe/London), from `date_trunc('day', recorded_at)`.
    pub day: chrono::NaiveDate,
    /// Percentage of observations on time that day, 0–100.
    pub on_time_pct: Option<f64>,
    /// Mean delay in minutes that day.
    pub avg_delay_mins: Option<f64>,
    /// Number of observations that day.
    pub sample_count: i64,
}

/// Per-day punctuality trend for one operator over the last `hours` hours,
/// ordered oldest → newest.
///
/// Days are bucketed in the `Europe/London` zone so the boundary matches the
/// UK operating day rather than UTC midnight.
pub async fn operator_daily_series(
    db: &Db,
    toc: &str,
    hours: i32,
) -> sqlx::Result<Vec<OperatorDailyPoint>> {
    sqlx::query_as::<_, OperatorDailyPoint>(
        r#"
        SELECT
            date_trunc('day', d.recorded_at AT TIME ZONE 'Europe/London')::date  AS day,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            AVG(d.delay_mins::float8)                                             AS avg_delay_mins,
            COUNT(*)                                                              AS sample_count
        FROM delay_history d
        JOIN services s ON s.uid = d.uid
        WHERE s.toc = $1
          AND d.recorded_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        GROUP BY date_trunc('day', d.recorded_at AT TIME ZONE 'Europe/London')::date
        ORDER BY day ASC
        "#,
    )
    .bind(toc)
    .bind(hours)
    .fetch_all(db)
    .await
}

/// One delay band of the histogram, identified by its inclusive lower bound.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct DelayBucket {
    /// Lower-bound sentinel for the band: 0 (on time, `<=0`), 1 (`0<x<=5`),
    /// 6 (`5<x<=15`), 16 (`15<x<=30`), 31 (`30<x<=60`), 61 (`>60`).
    pub lower_bound_mins: i32,
    /// Number of observations in this band.
    pub sample_count: i64,
}

/// Delay-distribution histogram for one operator over the last `hours` hours.
///
/// Buckets `delay_mins` into fixed bands `(-∞,0] (0,5] (5,15] (15,30] (30,60] (60,∞)`
/// and returns one row per non-empty band, ascending by lower bound. The
/// `lower_bound_mins` sentinel encodes the band edge (0/1/6/16/31/61).
pub async fn operator_delay_distribution(
    db: &Db,
    toc: &str,
    hours: i32,
) -> sqlx::Result<Vec<DelayBucket>> {
    sqlx::query_as::<_, DelayBucket>(
        r#"
        SELECT
            CASE
                WHEN d.delay_mins <= 0  THEN 0
                WHEN d.delay_mins <= 5  THEN 1
                WHEN d.delay_mins <= 15 THEN 6
                WHEN d.delay_mins <= 30 THEN 16
                WHEN d.delay_mins <= 60 THEN 31
                ELSE 61
            END                AS lower_bound_mins,
            COUNT(*)           AS sample_count
        FROM delay_history d
        JOIN services s ON s.uid = d.uid
        WHERE s.toc = $1
          AND d.recorded_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        GROUP BY lower_bound_mins
        ORDER BY lower_bound_mins ASC
        "#,
    )
    .bind(toc)
    .bind(hours)
    .fetch_all(db)
    .await
}

/// One route (origin → destination) of an operator's network with its punctuality.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct RouteRow {
    /// Origin CRS (`services.origin_crs`).
    pub origin_crs: String,
    /// Destination CRS (`services.destination_crs`).
    pub destination_crs: String,
    /// Percentage of observations on this route on time, 0–100.
    pub on_time_pct: Option<f64>,
    /// Mean delay in minutes on this route.
    pub avg_delay_mins: Option<f64>,
    /// Number of observations on this route.
    pub sample_count: i64,
}

/// Per-route punctuality for one operator over the last `hours` hours,
/// worst (highest average delay) first, capped at `limit` rows.
///
/// Routes are the distinct `(origin_crs, destination_crs)` pairs drawn from
/// `services` — `delay_history` carries no destination, so the O–D pairing is
/// supplied entirely by the service join.
pub async fn operator_routes(
    db: &Db,
    toc: &str,
    hours: i32,
    limit: i64,
) -> sqlx::Result<Vec<RouteRow>> {
    sqlx::query_as::<_, RouteRow>(
        r#"
        SELECT
            s.origin_crs                                                          AS origin_crs,
            s.destination_crs                                                     AS destination_crs,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            AVG(d.delay_mins::float8)                                             AS avg_delay_mins,
            COUNT(*)                                                              AS sample_count
        FROM delay_history d
        JOIN services s ON s.uid = d.uid
        WHERE s.toc = $1
          AND d.recorded_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
        GROUP BY s.origin_crs, s.destination_crs
        ORDER BY avg_delay_mins DESC NULLS LAST
        LIMIT $3
        "#,
    )
    .bind(toc)
    .bind(hours)
    .bind(limit)
    .fetch_all(db)
    .await
}

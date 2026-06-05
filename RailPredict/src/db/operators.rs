//! Operator (TOC) league + per-operator drill-down, plus the `uid → toc` writer.
//!
//! The league and drill-down read from `journeys` — grouped by the per-service `toc`
//! captured from the Darwin `schedule` message — so no GTFS timetable is required. The
//! delay metric is arrival delay (what passengers experience), falling back to origin
//! departure delay when a service has no arrival observation. "On time" = within 5 minutes.
//!
//! `upsert_service_toc` separately persists the `uid → toc` mapping into `services`, so the
//! uid-keyed `delay_history` becomes operator-attributable too.

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
    sqlx::query_as::<_, Operator>("SELECT toc, name, brand_color FROM operators ORDER BY name")
        .fetch_all(db)
        .await
}

/// Persist a `uid → toc` mapping from the Darwin `schedule` message so `delay_history`
/// (keyed on `uid`) becomes operator-attributable. origin/destination are left NULL — the
/// schedule carries TIPLOCs, not CRS; the GTFS ingest fills them in if it ever runs.
pub async fn upsert_service_toc(db: &Db, uid: &str, toc: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO services (uid, toc) VALUES ($1, $2)
         ON CONFLICT (uid) DO UPDATE SET toc = EXCLUDED.toc, updated_at = now()",
    )
    .bind(uid)
    .bind(toc)
    .execute(db)
    .await?;
    Ok(())
}

/// One operator's punctuality aggregates over a rolling window — a league row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OperatorLeagueRow {
    pub toc: String,
    pub name: String,
    pub brand_color: String,
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    /// Mean absolute prediction error (minutes) over journeys with a finalised prediction.
    pub mae_mins: Option<f64>,
    pub sample_count: i64,
}

/// Operator league over the last `hours` hours, from `journeys` grouped by `toc`, ranked by
/// on-time % (arrival within 5 min; origin departure delay as fallback). Only operators with
/// at least `min_samples` journeys are included. Coverage grows as `schedule` messages accrue.
pub async fn operator_league(
    db: &Db,
    hours: i32,
    min_samples: i64,
    limit: i64,
) -> sqlx::Result<Vec<OperatorLeagueRow>> {
    sqlx::query_as::<_, OperatorLeagueRow>(
        r#"
        SELECT
            j.toc                                                              AS toc,
            COALESCE(o.name, j.toc)                                            AS name,
            COALESCE(o.brand_color, '#9aa7b4')                                 AS brand_color,
            (AVG(CASE WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 5
                      THEN 1.0 ELSE 0.0 END) * 100)::float8                     AS on_time_pct,
            AVG(COALESCE(j.arrival_delay_mins, j.origin_delay_mins)::float8)    AS avg_delay_mins,
            (AVG(ABS(po.predicted_delay_mins - po.final_delay_mins))
                FILTER (WHERE po.final_delay_mins IS NOT NULL))::float8         AS mae_mins,
            COUNT(*)                                                           AS sample_count
        FROM journeys j
        LEFT JOIN operators o ON o.toc = j.toc
        LEFT JOIN prediction_outcomes po ON po.rid = j.rid
        WHERE j.finalised_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND j.toc IS NOT NULL AND j.toc <> ''
          AND COALESCE(j.arrival_delay_mins, j.origin_delay_mins) BETWEEN -120 AND 600
        GROUP BY j.toc, o.name, o.brand_color
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
// Per-operator drill-down (the `/operators/:toc` page) — all from `journeys`.
// ---------------------------------------------------------------------------

/// Headline aggregates for one operator over a rolling window.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct OperatorDetail {
    pub toc: String,
    pub name: String,
    pub brand_color: String,
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub mae_mins: Option<f64>,
    pub sample_count: i64,
}

/// Single-operator headline aggregates over the last `hours` hours. `None` when the operator
/// has no journeys in the window.
pub async fn operator_detail(
    db: &Db,
    toc: &str,
    hours: i32,
) -> sqlx::Result<Option<OperatorDetail>> {
    sqlx::query_as::<_, OperatorDetail>(
        r#"
        SELECT
            j.toc                                                              AS toc,
            COALESCE(o.name, j.toc)                                            AS name,
            COALESCE(o.brand_color, '#9aa7b4')                                 AS brand_color,
            (AVG(CASE WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 5
                      THEN 1.0 ELSE 0.0 END) * 100)::float8                     AS on_time_pct,
            AVG(COALESCE(j.arrival_delay_mins, j.origin_delay_mins)::float8)    AS avg_delay_mins,
            (AVG(ABS(po.predicted_delay_mins - po.final_delay_mins))
                FILTER (WHERE po.final_delay_mins IS NOT NULL))::float8         AS mae_mins,
            COUNT(*)                                                           AS sample_count
        FROM journeys j
        LEFT JOIN operators o ON o.toc = j.toc
        LEFT JOIN prediction_outcomes po ON po.rid = j.rid
        WHERE j.toc = $1
          AND j.finalised_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND COALESCE(j.arrival_delay_mins, j.origin_delay_mins) BETWEEN -120 AND 600
        GROUP BY j.toc, o.name, o.brand_color
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
    pub day: chrono::NaiveDate,
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub sample_count: i64,
}

/// Per-day punctuality trend for one operator over the last `hours` hours (oldest → newest),
/// bucketed in `Europe/London` so day boundaries match the UK operating day.
pub async fn operator_daily_series(
    db: &Db,
    toc: &str,
    hours: i32,
) -> sqlx::Result<Vec<OperatorDailyPoint>> {
    sqlx::query_as::<_, OperatorDailyPoint>(
        r#"
        SELECT
            date_trunc('day', j.finalised_at AT TIME ZONE 'Europe/London')::date AS day,
            (AVG(CASE WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 5
                      THEN 1.0 ELSE 0.0 END) * 100)::float8                       AS on_time_pct,
            AVG(COALESCE(j.arrival_delay_mins, j.origin_delay_mins)::float8)      AS avg_delay_mins,
            COUNT(*)                                                             AS sample_count
        FROM journeys j
        WHERE j.toc = $1
          AND j.finalised_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND COALESCE(j.arrival_delay_mins, j.origin_delay_mins) BETWEEN -120 AND 600
        GROUP BY date_trunc('day', j.finalised_at AT TIME ZONE 'Europe/London')::date
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
    /// Lower-bound sentinel: 0 (`<=0`), 1 (`0<x<=5`), 6 (`5<x<=15`), 16 (`15<x<=30`),
    /// 31 (`30<x<=60`), 61 (`>60`).
    pub lower_bound_mins: i32,
    pub sample_count: i64,
}

/// Delay-distribution histogram for one operator over the last `hours` hours, over the
/// arrival delay (origin fallback). One row per non-empty band, ascending by lower bound.
pub async fn operator_delay_distribution(
    db: &Db,
    toc: &str,
    hours: i32,
) -> sqlx::Result<Vec<DelayBucket>> {
    sqlx::query_as::<_, DelayBucket>(
        r#"
        SELECT
            CASE
                WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 0  THEN 0
                WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 5  THEN 1
                WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 15 THEN 6
                WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 30 THEN 16
                WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 60 THEN 31
                ELSE 61
            END                AS lower_bound_mins,
            COUNT(*)           AS sample_count
        FROM journeys j
        WHERE j.toc = $1
          AND j.finalised_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND COALESCE(j.arrival_delay_mins, j.origin_delay_mins) BETWEEN -120 AND 600
        GROUP BY lower_bound_mins
        ORDER BY lower_bound_mins ASC
        "#,
    )
    .bind(toc)
    .bind(hours)
    .fetch_all(db)
    .await
}

/// One route (origin → destination TIPLOC) of an operator's network with its punctuality.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct RouteRow {
    /// Origin TIPLOC (`journeys.origin_tpl`). (Name resolution happens in the view layer.)
    pub origin_crs: String,
    /// Destination TIPLOC (`journeys.destination_tpl`).
    pub destination_crs: String,
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub sample_count: i64,
}

/// Per-route punctuality for one operator over the last `hours` hours, worst (highest average
/// delay) first, capped at `limit`. Routes are the `(origin_tpl, destination_tpl)` pairs.
pub async fn operator_routes(
    db: &Db,
    toc: &str,
    hours: i32,
    limit: i64,
) -> sqlx::Result<Vec<RouteRow>> {
    sqlx::query_as::<_, RouteRow>(
        r#"
        SELECT
            j.origin_tpl                                                         AS origin_crs,
            COALESCE(j.destination_tpl, '')                                      AS destination_crs,
            (AVG(CASE WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 5
                      THEN 1.0 ELSE 0.0 END) * 100)::float8                       AS on_time_pct,
            AVG(COALESCE(j.arrival_delay_mins, j.origin_delay_mins)::float8)      AS avg_delay_mins,
            COUNT(*)                                                             AS sample_count
        FROM journeys j
        WHERE j.toc = $1
          AND j.finalised_at > NOW() - $2::INT * INTERVAL '1 hour'
          AND COALESCE(j.arrival_delay_mins, j.origin_delay_mins) BETWEEN -120 AND 600
        GROUP BY j.origin_tpl, j.destination_tpl
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

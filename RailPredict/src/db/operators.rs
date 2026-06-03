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

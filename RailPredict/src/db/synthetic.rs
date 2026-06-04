use super::Db;

/// Aggregate stats for the most recent synthetic-data generation, for the
/// "synthetic data" card on the predictions page.
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct SyntheticStats {
    pub generation: String,
    pub total_rows: i64,
    pub good_rows: i64,
    pub average_rows: i64,
    pub avg_delay_good: f64,
    pub avg_delay_average: f64,
    pub ontime_pct_good: f64,
    pub ontime_pct_average: f64,
}

/// Latest synthetic generation's row counts and per-day-type delay/on-time stats.
/// Returns `None` when no synthetic data has been generated yet.
///
/// Uses runtime `query_as` (not the `query!` macro) so the build stays decoupled
/// from a live/offline schema, matching every other query in this layer. NULL
/// aggregates are coalesced to `0.0` in SQL so the DTO fields stay non-optional.
pub async fn synthetic_stats(db: &Db) -> sqlx::Result<Option<SyntheticStats>> {
    sqlx::query_as::<_, SyntheticStats>(
        r#"
        SELECT
            generation                                                            AS generation,
            COUNT(*)                                                              AS total_rows,
            COUNT(*) FILTER (WHERE day_type = 'good')                             AS good_rows,
            COUNT(*) FILTER (WHERE day_type = 'average')                          AS average_rows,
            COALESCE(AVG(delay_mins::float8) FILTER (WHERE day_type = 'good'),    0)::float8 AS avg_delay_good,
            COALESCE(AVG(delay_mins::float8) FILTER (WHERE day_type = 'average'), 0)::float8 AS avg_delay_average,
            COALESCE(
                100.0 * COUNT(*) FILTER (WHERE day_type = 'good'    AND delay_mins <= 0)::float8
                      / NULLIF(COUNT(*) FILTER (WHERE day_type = 'good'),    0)::float8,
                0)::float8                                                        AS ontime_pct_good,
            COALESCE(
                100.0 * COUNT(*) FILTER (WHERE day_type = 'average' AND delay_mins <= 0)::float8
                      / NULLIF(COUNT(*) FILTER (WHERE day_type = 'average'), 0)::float8,
                0)::float8                                                        AS ontime_pct_average
        FROM delay_history_synthetic
        GROUP BY generation
        ORDER BY generation DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(db)
    .await
}

use super::Db;

#[derive(Debug, serde::Serialize)]
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

pub async fn synthetic_stats(db: &Db) -> sqlx::Result<Option<SyntheticStats>> {
    let row = sqlx::query!(
        r#"
        SELECT
            generation,
            COUNT(*)                                                        AS "total_rows!: i64",
            COUNT(*) FILTER (WHERE day_type = 'good')                       AS "good_rows!: i64",
            COUNT(*) FILTER (WHERE day_type = 'average')                    AS "average_rows!: i64",
            AVG(delay_mins::float8) FILTER (WHERE day_type = 'good')        AS "avg_delay_good: f64",
            AVG(delay_mins::float8) FILTER (WHERE day_type = 'average')     AS "avg_delay_average: f64",
            100.0 * COUNT(*) FILTER (WHERE day_type = 'good'    AND delay_mins <= 0)::float8
                  / NULLIF(COUNT(*) FILTER (WHERE day_type = 'good'),    0)::float8
                                                                            AS "ontime_pct_good: f64",
            100.0 * COUNT(*) FILTER (WHERE day_type = 'average' AND delay_mins <= 0)::float8
                  / NULLIF(COUNT(*) FILTER (WHERE day_type = 'average'), 0)::float8
                                                                            AS "ontime_pct_average: f64"
        FROM delay_history_synthetic
        GROUP BY generation
        ORDER BY generation DESC
        LIMIT 1
        "#
    )
    .fetch_optional(db)
    .await?;

    Ok(row.map(|r| SyntheticStats {
        generation:         r.generation,
        total_rows:         r.total_rows,
        good_rows:          r.good_rows,
        average_rows:       r.average_rows,
        avg_delay_good:     r.avg_delay_good.unwrap_or(0.0),
        avg_delay_average:  r.avg_delay_average.unwrap_or(0.0),
        ontime_pct_good:    r.ontime_pct_good.unwrap_or(0.0),
        ontime_pct_average: r.ontime_pct_average.unwrap_or(0.0),
    }))
}

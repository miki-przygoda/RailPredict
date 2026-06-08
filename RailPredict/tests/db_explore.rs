//! Tests for the Query Explorer engine (`db/explore.rs`): representative spec
//! combinations across all three subjects produce correct aggregates, validation
//! clamps/whitelists raw input, and the guards (row cap, default window) hold.

use railpredict::db::explore::{run_explore, ExploreResult, ExploreSpec, GroupBy, Metric, RawExplore, Subject};

fn row<'a>(res: &'a ExploreResult, label: &str) -> Option<&'a Vec<String>> {
    res.rows.iter().find(|r| r[0] == label)
}

/// Two operators (HX, GW) with delay observations across hours.
async fn seed_observations(pool: &sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs,name)
         VALUES ('PAD','London Paddington'),('HXX','Heathrow'),('RDG','Reading')
         ON CONFLICT (crs) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO services (uid,origin_crs,destination_crs,toc)
         VALUES ('HX0001','PAD','HXX','HX'),('GW0001','PAD','RDG','GW')
         ON CONFLICT (uid) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    // HX: hour 7 (+2), hour 8 (+4), hour 9 (on time). GW: hour 14 (+10). weekday 2 = Wed.
    sqlx::query(
        "INSERT INTO delay_history (uid,weekday,origin_crs,delay_mins,departure_hour,recorded_at)
         VALUES ('HX0001',2,'PAD',2,7,NOW()),
                ('HX0001',2,'PAD',4,8,NOW()),
                ('HX0001',2,'PAD',0,9,NOW()),
                ('GW0001',2,'PAD',10,14,NOW())",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../migrations")]
async fn observations_filter_and_group_by_hour(pool: sqlx::PgPool) {
    seed_observations(&pool).await;
    // "avg delay for observations 06:00–10:00 where operator = HX, grouped by hour"
    let spec = ExploreSpec::from_raw(RawExplore {
        window: Some("7d"),
        from_hour: Some(6),
        to_hour: Some(10),
        operator: Some("HX"),
        group: Some("hour"),
        metric: Some("avg_delay"),
        ..Default::default()
    });
    let res = run_explore(&pool, &spec).await.unwrap();
    assert_eq!(res.columns[0], "Hour");
    assert_eq!(row(&res, "07:00").expect("07:00")[1], "2.00");
    assert_eq!(row(&res, "08:00").expect("08:00")[1], "4.00");
    assert!(row(&res, "14:00").is_none(), "GW 14:00 must be filtered out");
}

#[sqlx::test(migrations = "../migrations")]
async fn observations_median_percentile(pool: sqlx::PgPool) {
    seed_observations(&pool).await;
    // HX delays 0,2,4 → median (p50) = 2
    let spec = ExploreSpec::from_raw(RawExplore {
        operator: Some("HX"),
        group: Some("none"),
        metric: Some("p50"),
        ..Default::default()
    });
    let res = run_explore(&pool, &spec).await.unwrap();
    assert_eq!(res.rows[0][0], "All");
    assert_eq!(res.rows[0][1], "2.00");
}

#[sqlx::test(migrations = "../migrations")]
async fn predictions_mae_by_operator(pool: sqlx::PgPool) {
    sqlx::query("INSERT INTO stations (crs,name) VALUES ('PAD','London Paddington') ON CONFLICT (crs) DO NOTHING")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO services (uid,origin_crs,destination_crs,toc) VALUES ('HX0001','PAD','PAD','HX') ON CONFLICT (uid) DO NOTHING")
        .execute(&pool).await.unwrap();
    // Two finalised predictions for HX: errors |5-3|=2 and |4-4|=0 → MAE 1.0
    sqlx::query(
        "INSERT INTO prediction_outcomes
            (rid, uid, origin_crs, scheduled_departure, predicted_delay_mins, final_delay_mins, finalised_at, prediction_confidence)
         VALUES ('RID000000000001','HX0001','PAD',NOW(),3,5,NOW(),0.8),
                ('RID000000000002','HX0001','PAD',NOW(),4,4,NOW(),0.7)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let spec = ExploreSpec::from_raw(RawExplore {
        subject: Some("predictions"),
        group: Some("operator"),
        metric: Some("mae"),
        ..Default::default()
    });
    let res = run_explore(&pool, &spec).await.unwrap();
    assert_eq!(spec.subject, Subject::Predictions);
    assert_eq!(row(&res, "HX").expect("HX")[1], "1.00");
}

#[sqlx::test(migrations = "../migrations")]
async fn cancellations_count_by_hour(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO cancellations (uid,origin_crs,weekday,departure_hour,recorded_at)
         VALUES ('HX0001','PAD',2,7,NOW()),('HX0001','PAD',2,7,NOW()),('GW0001','RDG',2,14,NOW())",
    )
    .execute(&pool)
    .await
    .unwrap();
    let spec = ExploreSpec::from_raw(RawExplore {
        subject: Some("cancellations"),
        group: Some("hour"),
        metric: Some("count"),
        ..Default::default()
    });
    let res = run_explore(&pool, &spec).await.unwrap();
    assert_eq!(row(&res, "07:00").expect("07:00")[1], "2");
    assert_eq!(row(&res, "14:00").expect("14:00")[1], "1");
}

#[sqlx::test(migrations = "../migrations")]
async fn date_range_filter(pool: sqlx::PgPool) {
    seed_observations(&pool).await;
    // A from_date far in the future excludes everything.
    let spec = ExploreSpec::from_raw(RawExplore {
        group: Some("none"),
        metric: Some("count"),
        from_date: Some("2099-01-01"),
        window: Some("all"),
        ..Default::default()
    });
    let res = run_explore(&pool, &spec).await.unwrap();
    // No rows in the future → COUNT(*) over an empty set with no GROUP BY = single 0 row.
    assert_eq!(res.rows[0][1], "0");
}

#[test]
fn validation_clamps_whitelists_and_caps() {
    let spec = ExploreSpec::from_raw(RawExplore {
        subject: Some("cancellations"),
        window: Some("nonsense"), // → default 168
        from_hour: Some(99),      // → 23
        to_hour: Some(-5),        // → 0
        weekdays: Some("9,1,bad,3"), // → [1,3]
        operator: Some(""),       // → None
        origin: Some("toolong"),  // → None (not 3 letters)
        destination: Some("xy"),  // → None
        metric: Some("mae"),      // not valid for cancellations → default Count
        group: Some("wat"),       // → None
        limit: Some(99_999),      // → 500
        ..Default::default()
    });
    assert_eq!(spec.subject, Subject::Cancellations);
    assert_eq!(spec.window_hours, 168);
    assert_eq!(spec.from_hour, Some(23));
    assert_eq!(spec.to_hour, Some(0));
    assert_eq!(spec.weekdays, vec![1, 3]);
    assert!(spec.operator.is_none());
    assert!(spec.origin.is_none());
    assert!(spec.destination.is_none());
    assert_eq!(spec.group_by, GroupBy::None);
    assert_eq!(spec.metric, Metric::Count, "mae invalid for cancellations → Count");
    assert_eq!(spec.limit, 500);
}

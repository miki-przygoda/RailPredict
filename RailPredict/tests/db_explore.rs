//! Tests for the Query Explorer engine (`db/explore.rs`): representative spec
//! combinations produce correct aggregates, validation clamps/whitelists raw input,
//! and the guards (row cap, default window) hold.

use railpredict::db::explore::{run_explore, ExploreResult, ExploreSpec, GroupBy, Metric};

/// Two operators (HX, GW) with a few observations across hours.
async fn seed(pool: &sqlx::PgPool) {
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

fn row<'a>(res: &'a ExploreResult, label: &str) -> Option<&'a Vec<String>> {
    res.rows.iter().find(|r| r[0] == label)
}

#[sqlx::test(migrations = "../migrations")]
async fn filter_operator_and_hours_group_by_hour(pool: sqlx::PgPool) {
    seed(&pool).await;
    // "trains 06:00–10:00 where operator = HX, grouped by hour, avg delay"
    let spec = ExploreSpec::from_raw(
        Some("7d"), Some(6), Some(10), None, Some("HX"), None, None, None,
        Some("hour"), Some("avg_delay"), None,
    );
    let res = run_explore(&pool, &spec).await.unwrap();
    assert_eq!(res.columns[0], "Hour");
    assert_eq!(row(&res, "07:00").expect("07:00")[1], "2.0");
    assert_eq!(row(&res, "08:00").expect("08:00")[1], "4.0");
    assert_eq!(row(&res, "09:00").expect("09:00")[1], "0.0");
    // GW's 14:00 is excluded by BOTH the hour filter and the operator filter
    assert!(row(&res, "14:00").is_none(), "14:00 should be filtered out");
}

#[sqlx::test(migrations = "../migrations")]
async fn group_none_count_is_total(pool: sqlx::PgPool) {
    seed(&pool).await;
    let spec = ExploreSpec::from_raw(
        Some("7d"), None, None, None, None, None, None, None, Some("none"), Some("count"), None,
    );
    let res = run_explore(&pool, &spec).await.unwrap();
    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0][0], "All");
    assert_eq!(res.rows[0][1], "4");
}

#[sqlx::test(migrations = "../migrations")]
async fn group_operator_on_time_pct(pool: sqlx::PgPool) {
    seed(&pool).await;
    let spec = ExploreSpec::from_raw(
        Some("7d"), None, None, None, None, None, None, None, Some("operator"), Some("on_time_pct"), None,
    );
    let res = run_explore(&pool, &spec).await.unwrap();
    // HX: 3 obs, 1 on time (delay 0) → 33.3%
    assert!(row(&res, "HX").expect("HX")[1].starts_with("33."), "HX on-time wrong");
    assert_eq!(row(&res, "GW").expect("GW")[1], "0.0");
}

#[sqlx::test(migrations = "../migrations")]
async fn list_returns_observations(pool: sqlx::PgPool) {
    seed(&pool).await;
    let spec = ExploreSpec::from_raw(
        Some("7d"), None, None, None, Some("HX"), None, None, None, Some("none"), Some("list"), None,
    );
    let res = run_explore(&pool, &spec).await.unwrap();
    assert_eq!(res.columns[0], "Service");
    assert_eq!(res.rows.len(), 3, "3 HX observations");
    assert!(res.rows.iter().all(|r| r[2] == "HX"), "operator column should be HX");
}

#[test]
fn validation_clamps_whitelists_and_caps() {
    let spec = ExploreSpec::from_raw(
        Some("nonsense"), // window → default 168
        Some(99),         // from_hour → clamp 23
        Some(-5),         // to_hour → clamp 0
        Some("9,1,bad,3"),// weekdays → [1,3] (9 out of range, 'bad' unparseable)
        Some(""),         // operator empty → None
        Some("toolong"),  // origin not 3 letters → None
        Some("xy"),       // destination not 3 letters → None
        None,
        Some("wat"),      // unknown group → None
        Some("wat"),      // unknown metric → List
        Some(99_999),     // limit → capped at 500
    );
    assert_eq!(spec.window_hours, 168);
    assert_eq!(spec.from_hour, Some(23));
    assert_eq!(spec.to_hour, Some(0));
    assert_eq!(spec.weekdays, vec![1, 3]);
    assert!(spec.operator.is_none());
    assert!(spec.origin.is_none());
    assert!(spec.destination.is_none());
    assert_eq!(spec.group_by, GroupBy::None);
    assert_eq!(spec.metric, Metric::List);
    assert_eq!(spec.limit, 500);
}

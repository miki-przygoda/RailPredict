//! Tests for `build_departure_board` — the DB-timetable × live-registry merge
//! (api/handlers.rs), the one untested non-trivial logic path. Driven through the
//! JSON endpoint `GET /stations/:crs/departures` so it doubles as an HTTP contract.

mod http_common;
use http_common::spawn_app_empty;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, Utc};
use railpredict::types::{Stamped, TrainId, TrainStatus};
use reqwest::StatusCode;

async fn seed_timetable(pool: &sqlx::PgPool, date: NaiveDate) {
    sqlx::query(
        "INSERT INTO stations (crs,name) VALUES ('LDS','Leeds'),('MAN','Manchester Piccadilly')
         ON CONFLICT (crs) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO services (uid,origin_crs,destination_crs) VALUES ('C12345','LDS','MAN')
         ON CONFLICT (uid) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    let dep = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    sqlx::query(
        "INSERT INTO timetable_calls
            (uid,operating_date,location_crs,call_order,scheduled_departure,public_departure,platform)
         VALUES ('C12345',$1,'LDS',0,$2,$2,'1')",
    )
    .bind(date)
    .bind(dep)
    .execute(pool)
    .await
    .unwrap();
}

/// DB timetable, empty registry → entry built from the timetable: rid falls back
/// to the uid, no live delay, platform marked planned.
#[sqlx::test(migrations = "../migrations")]
async fn db_only_entry_from_timetable(pool: sqlx::PgPool) {
    let date = Utc::now().date_naive();
    seed_timetable(&pool, date).await;
    let app = spawn_app_empty(pool).await;

    let (status, json) = app.get_json("/stations/LDS/departures").await;
    assert_eq!(status, StatusCode::OK);
    let arr = json.as_array().expect("array of entries");
    let e = arr.iter().find(|e| e["rid"].as_str().map(str::trim) == Some("C12345"))
        .expect("timetable entry present");
    assert!(e["delay_mins"].is_null(), "no live delay expected");
    assert_eq!(e["is_platform_planned"].as_bool(), Some(true), "platform from timetable");
}

/// A live registry entry at the same scheduled time overlays the DB row: the live
/// rid/delay/platform win and the platform is no longer "planned".
#[sqlx::test(migrations = "../migrations")]
async fn registry_overlay_wins_within_window(pool: sqlx::PgPool) {
    let date = Utc::now().date_naive();
    seed_timetable(&pool, date).await;
    let app = spawn_app_empty(pool).await;

    let sched = NaiveDateTime::new(date, NaiveTime::from_hms_opt(9, 0, 0).unwrap()).and_utc();
    let rid = TrainId::rid("202406040900001").unwrap();
    let mut ts = TrainStatus::new(rid.clone(), sched, sched);
    ts.origin_crs = Some("LDS".to_string());
    ts.reported_delay_mins = Stamped::new(Some(5));
    ts.actual_platform = Stamped::new(Some("9".to_string()));
    app.registry.upsert(rid.clone(), ts);

    let (status, json) = app.get_json("/stations/LDS/departures").await;
    assert_eq!(status, StatusCode::OK);
    let arr = json.as_array().unwrap();
    let e = arr
        .iter()
        .find(|e| e["rid"].as_str() == Some("202406040900001"))
        .unwrap_or_else(|| panic!("live entry should overlay the DB row: {json}"));
    assert_eq!(e["delay_mins"].as_i64(), Some(5));
    assert_eq!(e["is_platform_planned"].as_bool(), Some(false), "live platform");
}

/// No timetable rows → the board degrades gracefully to the registry snapshot.
#[sqlx::test(migrations = "../migrations")]
async fn empty_timetable_degrades_to_registry(pool: sqlx::PgPool) {
    let app = spawn_app_empty(pool).await;

    let sched = Utc::now() + chrono::Duration::minutes(20);
    let rid = TrainId::rid("202406040900002").unwrap();
    let mut ts = TrainStatus::new(rid.clone(), sched, sched);
    ts.origin_crs = Some("LDS".to_string());
    app.registry.upsert(rid.clone(), ts);

    let (status, json) = app.get_json("/stations/LDS/departures").await;
    assert_eq!(status, StatusCode::OK);
    let arr = json.as_array().unwrap();
    assert!(
        arr.iter().any(|e| e["rid"].as_str() == Some("202406040900002")),
        "registry-only degrade expected: {json}"
    );
}

//! End-to-end HTTP test for the station connection / journey finder.
//!
//! Drives the *real* axum app over `reqwest` (zero new deps) to prove the finder
//! works server-side: autocomplete → /ui/journeys → rendered board, plus the
//! validation + no-service edges and the departure board.

mod http_common;
use http_common::spawn_app;

use chrono::{NaiveTime, Utc};
use reqwest::StatusCode;

/// Seed an LDS→MAN through-service for *today* (the HTTP layer defaults to today).
async fn seed(pool: &sqlx::PgPool) {
    let date = Utc::now().date_naive();
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ('LDS','Leeds'),('MAN','Manchester Piccadilly')
         ON CONFLICT (crs) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO services (uid, origin_crs, destination_crs) VALUES ('C12345','LDS','MAN')
         ON CONFLICT (uid) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    let dep = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let arr = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
    sqlx::query(
        "INSERT INTO timetable_calls
            (uid, operating_date, location_crs, call_order, scheduled_departure, public_departure, platform)
         VALUES ('C12345',$1,'LDS',0,$2,$2,'1'), ('C12345',$1,'MAN',1,$3,$3,'2')",
    )
    .bind(date)
    .bind(dep)
    .bind(arr)
    .execute(pool)
    .await
    .unwrap();
}

fn stations() -> Vec<(String, String)> {
    vec![
        ("LDS".into(), "Leeds".into()),
        ("MAN".into(), "Manchester Piccadilly".into()),
    ]
}

#[sqlx::test(migrations = "../migrations")]
async fn autocomplete_returns_seeded_station(pool: sqlx::PgPool) {
    seed(&pool).await;
    let app = spawn_app(pool, stations()).await;

    let (status, body) = app.get("/ui/stations/search?q=lee").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#"data-crs="LDS""#), "missing LDS suggestion: {body}");
    assert!(body.contains("Leeds"));
    assert!(body.contains("suggestion-item"));

    // < 2 chars → empty fragment (no autocomplete)
    let (_, short) = app.get("/ui/stations/search?q=l").await;
    assert!(short.trim().is_empty(), "expected empty for 1-char query, got: {short}");
}

#[sqlx::test(migrations = "../migrations")]
async fn journey_finder_finds_through_service(pool: sqlx::PgPool) {
    seed(&pool).await;
    let app = spawn_app(pool, stations()).await;

    let (status, body) = app.get("/ui/journeys?from=LDS&to=MAN").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("departure-board"), "no board rendered: {body}");
    assert!(body.contains("C12345"), "through service missing: {body}");
    assert!(body.contains("09:00"), "departure time missing: {body}");
    assert!(body.contains("Manchester Piccadilly"), "destination missing");
}

#[sqlx::test(migrations = "../migrations")]
async fn journey_finder_validates_and_handles_no_service(pool: sqlx::PgPool) {
    seed(&pool).await;
    let app = spawn_app(pool, stations()).await;

    // same station → validation message
    let (_, same) = app.get("/ui/journeys?from=LDS&to=LDS").await;
    assert!(
        same.contains("Please enter valid origin and destination station codes."),
        "same-station got: {same}"
    );

    // bad CRS length → validation message
    let (_, bad) = app.get("/ui/journeys?from=LD&to=MAN").await;
    assert!(bad.contains("Please enter valid origin and destination station codes."));

    // reverse direction has no qualifying through service
    let (_, rev) = app.get("/ui/journeys?from=MAN&to=LDS").await;
    assert!(rev.contains("No direct services found"), "reverse got: {rev}");
}

#[sqlx::test(migrations = "../migrations")]
async fn departure_board_renders_for_station(pool: sqlx::PgPool) {
    seed(&pool).await;
    let app = spawn_app(pool, stations()).await;

    let (status, body) = app.get("/ui/stations/departures?crs=LDS").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("departure-board"), "no board: {body}");
    assert!(body.contains("C12345"), "service missing from board: {body}");
}

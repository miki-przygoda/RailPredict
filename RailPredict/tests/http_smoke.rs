//! HTTP smoke tests — each product page + key endpoint responds correctly.
//! Zero new deps: real app on an ephemeral port, driven via reqwest.

mod http_common;
use http_common::{spawn_app_empty, spawn_app_with_feed};
use reqwest::StatusCode;

#[sqlx::test(migrations = "../migrations")]
async fn pages_and_endpoints_respond(pool: sqlx::PgPool) {
    let app = spawn_app_empty(pool).await;

    // Dashboard
    let (s, b) = app.get("/").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Network Overview"), "dashboard markup unexpected");

    // Search / journey finder page
    let (s, b) = app.get("/search").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Where are you going?"));

    // Predictions explorer (predicted-vs-actual analytics)
    let (s, b) = app.get("/predictions").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Predicted vs actual"), "predictions explorer markup");
    assert!(b.contains("Calibration"), "calibration panel present");

    // Diagnostics console moved to /dev; /demo now serves the OS demo (checked below).
    let (s, b) = app.get("/dev").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Diagnostics"), "dev console markup unexpected");
    assert!(!b.contains("Tier C"), "tier framing removed");
    assert!(!b.contains("Confirm & Pay"), "simulated purchase removed");

    // /demo now renders the self-contained "RailPredict OS" demo desktop.
    let (s, _) = app.get("/demo").await;
    assert_eq!(s, StatusCode::OK, "/demo serves the OS demo");

    // Top nav no longer advertises the dev console; Operators is now linked.
    let (_s, home) = app.get("/").await;
    assert!(home.contains(">Overview<"), "nav has Overview");
    assert!(home.contains(">Operators<"), "nav has Operators");
    assert!(home.contains(">Live<"), "nav has Live");
    assert!(!home.contains("Dev Console"), "nav no longer shows Dev Console");

    // Live board shell + its JSON snapshot feed.
    let (s, b) = app.get("/live").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Live Network"), "live board shell");
    assert!(b.contains("/static/board.js"), "loads the shared board renderer");

    let (s, j) = app.get_json("/ui/live/snapshot").await;
    assert_eq!(s, StatusCode::OK);
    assert!(j["tracking"].is_array(), "snapshot has a tracking array");
    assert!(j["settled"].is_array(), "snapshot has a settled array");

    // Stations: index, explorer (empty-state on an empty DB), 404 on a too-long code.
    assert!(home.contains(">Stations<"), "nav has Stations");
    let (s, b) = app.get("/stations").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Filter by name or code"), "station index filter");

    let (s, _) = app.get("/stations/WATRLMN").await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = app.get("/stations/toolongcode").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "invalid code rejected");

    // Standalone replay page + its scripts are served (board.js shared with /live).
    let (s, _) = app.get("/static/replay.html").await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = app.get("/static/board.js").await;
    assert_eq!(s, StatusCode::OK);

    // Operators league (empty data in the test DB → branded empty-state, still 200).
    let (s, b) = app.get("/operators").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Punctuality league"), "operators league markup");

    // Operator drill-down: a well-formed TOC renders (empty-state with no data).
    let (s, _) = app.get("/operators/VT").await;
    assert_eq!(s, StatusCode::OK);

    // Malformed TOC path → 404.
    let (s, _) = app.get("/operators/toolong").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "invalid TOC rejected");

    // Health JSON (DB reachable via the test pool → "ok")
    let (s, j) = app.get_json("/health").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(j["status"], "ok");
    assert!(j["version"].is_string());

    // Valid 15-char but unknown RID → 404 JSON
    let (s, _) = app.get("/trains/202406040000000").await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Embedded static asset is served with the right MIME
    let resp = app
        .client
        .get(format!("{}/static/style.css", app.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text/css"), "static content-type: {ct}");
}

/// `/health` must not report healthy when the DB is fine but the Darwin feed is dead.
#[sqlx::test(migrations = "../migrations")]
async fn health_reports_stale_darwin_feed_as_degraded(pool: sqlx::PgPool) {
    use std::sync::Arc;
    use chrono::{Duration, Utc};
    use railpredict::ingestion::feed_health::FeedHealth;

    // Started an hour ago, last message 10 minutes ago, 5-minute threshold.
    let feed = Arc::new(FeedHealth::new_at(300, Utc::now() - Duration::hours(1)));
    feed.record_message_at(Utc::now() - Duration::minutes(10));
    let app = spawn_app_with_feed(pool.clone(), Vec::new(), Arc::clone(&feed)).await;

    let (s, j) = app.get_json("/health").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(j["status"], "degraded");
    assert_eq!(j["db"], "ok");
    assert_eq!(j["feed"], "stale");
    assert!(j["feed_lag_secs"].as_f64().unwrap() >= 600.0);

    // A message arrives: the same app is healthy again.
    feed.record_message();
    let (s, j) = app.get_json("/health").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(j["feed"], "fresh");
}

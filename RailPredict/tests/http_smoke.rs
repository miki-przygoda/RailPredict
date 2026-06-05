//! HTTP smoke tests — each product page + key endpoint responds correctly.
//! Zero new deps: real app on an ephemeral port, driven via reqwest.

mod http_common;
use http_common::spawn_app_empty;
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

    // Diagnostics console moved to /dev; the old /demo route is gone.
    let (s, b) = app.get("/dev").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("Diagnostics"), "dev console markup unexpected");
    assert!(!b.contains("Tier C"), "tier framing removed");
    assert!(!b.contains("Confirm & Pay"), "simulated purchase removed");

    let (s, _) = app.get("/demo").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "/demo retired");

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
    assert!(b.contains("Filter by code"), "station index filter");

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

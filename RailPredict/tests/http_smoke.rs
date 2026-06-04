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

    // Predictions analytics page
    let (s, b) = app.get("/predictions").await;
    assert_eq!(s, StatusCode::OK);
    assert!(b.contains("ML Predictions"));

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

//! HTTP test for the Query Explorer page: it renders the builder form (operator
//! dropdown populated) and a composed query runs end-to-end through the real app.

mod http_common;
use http_common::spawn_app_empty;
use reqwest::StatusCode;

async fn seed(pool: &sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs,name) VALUES ('PAD','London Paddington'),('HXX','Heathrow')
         ON CONFLICT (crs) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO services (uid,origin_crs,destination_crs,toc)
         VALUES ('HX0001','PAD','HXX','HX') ON CONFLICT (uid) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO operators (toc,name,brand_color)
         VALUES ('HX','Heathrow Express','#532a45') ON CONFLICT (toc) DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO delay_history (uid,weekday,origin_crs,delay_mins,departure_hour,recorded_at)
         VALUES ('HX0001',2,'PAD',2,7,NOW()),('HX0001',2,'PAD',4,8,NOW())",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../migrations")]
async fn explore_page_renders_and_runs_composed_query(pool: sqlx::PgPool) {
    seed(&pool).await;
    let app = spawn_app_empty(pool).await;

    // Bare page: the builder form + the operator dropdown from the operators table.
    let (status, body) = app.get("/explore").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Query Explorer"));
    assert!(body.contains("explore-form"));
    assert!(body.contains("Heathrow Express"), "operator dropdown should be populated");

    // Composed query: avg delay, operator HX, hours 6–10, grouped by hour.
    let (status, body) = app
        .get("/explore?metric=avg_delay&group=hour&operator=HX&from_hour=6&to_hour=10&window=7d")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("explore-table"), "expected a result table");
    assert!(body.contains("07:00"), "hour 07:00 row missing");
    assert!(body.contains("08:00"), "hour 08:00 row missing");
}

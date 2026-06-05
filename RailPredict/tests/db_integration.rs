//! Database integration tests for RailPredict.
//!
//! These tests exercise `src/db/history.rs` and `src/db/static_data.rs` against a real,
//! schema-migrated Postgres database. Each test function gets a freshly migrated database
//! via the `#[sqlx::test]` macro — no shared state between tests.
//!
//! # Running locally
//!
//! ```bash
//! # From repo root with Docker Compose running:
//! docker compose up -d db
//! cd RailPredict
//! DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict \
//!   cargo test --test db_integration
//! ```
//!
//! In CI the Postgres service container in `.github/workflows/ci.yml` provides the
//! database automatically; `DATABASE_URL` is set in the workflow environment.
//!
//! # Dependency note
//!
//! `sqlx` is already listed in `[dependencies]` in `Cargo.toml` (version 0.8 with
//! `runtime-tokio-rustls`, `postgres`, `migrate`, and `chrono` features), so it is
//! available for these tests without any `[dev-dependencies]` additions.
//!
//! The migrations path `"../migrations"` is relative to the crate root (`RailPredict/`)
//! and points to the `migrations/` directory one level up.
//!
//! # Compilation note
//!
//! Seed queries use the runtime `sqlx::query` (not `sqlx::query!`) so no database
//! connection is required at compile time and no `.sqlx/` offline snapshot is needed.

use std::sync::Arc;

use chrono::{NaiveDate, NaiveTime, Utc};

use railpredict::db;
use railpredict::prediction::types::{DelayRecord, HistoricalStore, ServicePattern};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_pattern(uid: &str) -> ServicePattern {
    ServicePattern {
        uid: uid.to_string(),
        weekday: chrono::Weekday::Mon,
        origin_crs: "LDS".to_string(),
        departure_hour: 9,
    }
}

// ---------------------------------------------------------------------------
// Test 1: load_history on a fresh (empty) database
//
// The delay_history table is empty after migrations; load_history should return
// a HistoricalStore with no patterns tracked.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn load_history_returns_empty_for_fresh_db(pool: sqlx::PgPool) {
    let store = db::history::load_history(&pool)
        .await
        .expect("load_history must not fail on a fresh database");

    let records = store.all_records();
    assert!(
        records.is_empty(),
        "Expected no records in a freshly migrated database, got {}",
        records.len()
    );
}

// ---------------------------------------------------------------------------
// Test 2: flush_history then load_history roundtrip
//
// Insert three delay records for two distinct patterns into an in-memory
// HistoricalStore, flush to the database, then reload from the database into
// a new HistoricalStore and verify the records survived the roundtrip.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn flush_then_load_roundtrips(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .bind("MAN")
    .bind("Manchester Piccadilly")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let store = Arc::new(HistoricalStore::new());

    let p1 = make_pattern("C12345");
    let p2 = ServicePattern {
        uid: "C67890".to_string(),
        weekday: chrono::Weekday::Fri,
        origin_crs: "MAN".to_string(),
        departure_hour: 17,
    };

    let base_time = Utc::now();
    let t1 = base_time - chrono::Duration::seconds(2);
    let t2 = base_time - chrono::Duration::seconds(1);
    let t3 = base_time;

    store.insert(p1.clone(), DelayRecord { delay_mins: 5, predicted_delay_mins: None, recorded_at: t1 });
    store.insert(p1.clone(), DelayRecord { delay_mins: 10, predicted_delay_mins: None, recorded_at: t2 });
    store.insert(p2.clone(), DelayRecord { delay_mins: 0, predicted_delay_mins: None, recorded_at: t3 });

    db::history::flush_history(&pool, &store)
        .await
        .expect("flush_history must not fail");

    let loaded = db::history::load_history(&pool)
        .await
        .expect("load_history must not fail after flush");

    let samples_p1 = loaded.get_samples(&p1).expect("p1 should have samples after reload");
    assert_eq!(samples_p1.len(), 2, "p1 must have 2 samples after roundtrip");
    assert!(samples_p1.contains(&5), "p1 samples must contain delay_mins=5");
    assert!(samples_p1.contains(&10), "p1 samples must contain delay_mins=10");

    let samples_p2 = loaded.get_samples(&p2).expect("p2 should have samples after reload");
    assert_eq!(samples_p2.len(), 1, "p2 must have 1 sample after roundtrip");
    assert_eq!(samples_p2[0], 0, "p2 sample must have delay_mins=0");
}

// ---------------------------------------------------------------------------
// Test 3: flush_history is idempotent (ON CONFLICT DO NOTHING)
//
// Flush the same records twice. The second flush must succeed without error
// and the loaded store must still have the same number of records, not double.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn flush_history_is_idempotent(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2) ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let store = Arc::new(HistoricalStore::new());
    let record = DelayRecord { delay_mins: 3, predicted_delay_mins: None, recorded_at: Utc::now() };
    store.insert(make_pattern("C12345"), record);

    db::history::flush_history(&pool, &store).await.expect("first flush must succeed");
    db::history::flush_history(&pool, &store).await.expect("second flush must succeed");

    let loaded = db::history::load_history(&pool).await.expect("load must succeed");
    let records = loaded.all_records();
    assert_eq!(
        records.len(),
        1,
        "idempotent flush must not duplicate records; got {}",
        records.len()
    );
}

// ---------------------------------------------------------------------------
// Test 4: get_station returns None for unknown CRS
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn get_station_returns_none_for_unknown_crs(pool: sqlx::PgPool) {
    let result = db::static_data::get_station(&pool, "ZZZ")
        .await
        .expect("get_station must not fail for missing CRS");
    assert!(result.is_none(), "Expected None for unknown CRS 'ZZZ'");
}

// ---------------------------------------------------------------------------
// Test 5: get_station returns inserted station
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn get_station_returns_inserted_station(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name, nlc, lat, lon, is_active)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind("KGX")
    .bind("London Kings Cross")
    .bind("5592")
    .bind(51.5308_f64)
    .bind(-0.1238_f64)
    .bind(true)
    .execute(&pool)
    .await
    .expect("station insert must succeed");

    let station = db::static_data::get_station(&pool, "KGX")
        .await
        .expect("get_station must not fail")
        .expect("station must be present");

    assert_eq!(station.crs.trim(), "KGX", "CRS must match");
    assert_eq!(station.name, "London Kings Cross", "Name must match");
    assert_eq!(station.nlc.as_deref().map(str::trim), Some("5592"), "NLC must match");
    assert!(station.is_active, "Station must be active");

    let lat = station.lat.expect("lat must be present");
    let lon = station.lon.expect("lon must be present");
    assert!((lat - 51.5308).abs() < 1e-4, "lat must be approximately correct");
    assert!((lon - (-0.1238)).abs() < 1e-4, "lon must be approximately correct");
}

// ---------------------------------------------------------------------------
// Test 6: departures_from returns empty for a station with no calls
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn departures_from_returns_empty_when_no_calls(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2) ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let date = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let calls = db::static_data::departures_from(&pool, "LDS", date)
        .await
        .expect("departures_from must not fail for empty timetable");

    assert!(calls.is_empty(), "Expected no calls for a freshly migrated database");
}

// ---------------------------------------------------------------------------
// Test 7: departures_from returns today's calls
//
// Insert a service, a station, and two timetable_calls rows for the same
// station on the same date. Verify both rows come back, ordered by
// scheduled_departure.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn departures_from_returns_todays_calls(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .bind("MAN")
    .bind("Manchester Piccadilly")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    sqlx::query(
        "INSERT INTO services (uid, origin_crs, destination_crs)
         VALUES ($1, $2, $3)
         ON CONFLICT (uid) DO NOTHING",
    )
    .bind("C12345")
    .bind("LDS")
    .bind("MAN")
    .execute(&pool)
    .await
    .expect("service seed must succeed");

    let operating_date = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let dep_0900 = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let dep_0930 = NaiveTime::from_hms_opt(9, 30, 0).unwrap();

    sqlx::query(
        "INSERT INTO timetable_calls
             (uid, operating_date, location_crs, call_order, scheduled_departure, public_departure, platform)
         VALUES ($1, $2, $3, $4, $5, $6, $7),
                ($1, $2, $3, $8, $9, $9, $10)",
    )
    .bind("C12345")       // $1
    .bind(operating_date) // $2
    .bind("LDS")          // $3
    .bind(0i16)           // $4 — first call_order
    .bind(dep_0900)       // $5 — scheduled_departure
    .bind(dep_0900)       // $6 — public_departure
    .bind("1")            // $7 — platform
    .bind(1i16)           // $8 — second call_order
    .bind(dep_0930)       // $9 — scheduled_departure + public_departure (reused)
    .bind("2A")           // $10 — platform
    .execute(&pool)
    .await
    .expect("timetable_calls insert must succeed");

    let calls = db::static_data::departures_from(&pool, "LDS", operating_date)
        .await
        .expect("departures_from must not fail");

    assert_eq!(calls.len(), 2, "Expected exactly 2 calls, got {}", calls.len());

    let first = &calls[0];
    let second = &calls[1];
    assert_eq!(first.scheduled_departure, Some(dep_0900), "First call must depart at 09:00");
    assert_eq!(second.scheduled_departure, Some(dep_0930), "Second call must depart at 09:30");
    assert_eq!(first.platform.as_deref(), Some("1"), "First call platform must be '1'");
    assert_eq!(second.platform.as_deref(), Some("2A"), "Second call platform must be '2A'");
    assert_eq!(first.uid.trim(), "C12345", "UID must match");
}

// ---------------------------------------------------------------------------
// Test 8: departures_from does not return calls from a different date
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn departures_from_excludes_other_dates(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .bind("MAN")
    .bind("Manchester Piccadilly")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    sqlx::query(
        "INSERT INTO services (uid, origin_crs, destination_crs)
         VALUES ($1, $2, $3) ON CONFLICT (uid) DO NOTHING",
    )
    .bind("C12345")
    .bind("LDS")
    .bind("MAN")
    .execute(&pool)
    .await
    .expect("service seed must succeed");

    let target_date = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let other_date = NaiveDate::from_ymd_opt(2024, 4, 18).unwrap();
    let dep_time = NaiveTime::from_hms_opt(10, 0, 0).unwrap();

    sqlx::query(
        "INSERT INTO timetable_calls
             (uid, operating_date, location_crs, call_order, scheduled_departure)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind("C12345")
    .bind(other_date)
    .bind("LDS")
    .bind(0i16)
    .bind(dep_time)
    .execute(&pool)
    .await
    .expect("timetable_calls insert must succeed");

    let calls = db::static_data::departures_from(&pool, "LDS", target_date)
        .await
        .expect("departures_from must not fail");

    assert!(
        calls.is_empty(),
        "departures_from must not return calls from a different date; got {}",
        calls.len()
    );
}

// ---------------------------------------------------------------------------
// Test 9: cheapest_fare returns None when no fares exist
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn cheapest_fare_returns_none_when_no_fares(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .bind("MAN")
    .bind("Manchester Piccadilly")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let today = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let result = db::static_data::cheapest_fare(&pool, "LDS", "MAN", today)
        .await
        .expect("cheapest_fare must not fail when fares table is empty");

    assert!(result.is_none(), "Expected None when fares table is empty");
}

// ---------------------------------------------------------------------------
// Test 10: cheapest_fare returns cheapest valid fare
//
// Insert three fares for the same origin/destination: two valid on the query
// date (at different prices) and one with an expired valid_to. Verify the
// cheapest of the two valid fares is returned.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn cheapest_fare_returns_cheapest_valid_fare(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .bind("MAN")
    .bind("Manchester Piccadilly")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let today = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let past = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    let future = NaiveDate::from_ymd_opt(2030, 12, 31).unwrap();
    let expired = NaiveDate::from_ymd_opt(2023, 1, 1).unwrap();

    // Three fares: 1000p (valid), 2500p (valid), 500p (expired — must be excluded).
    sqlx::query(
        "INSERT INTO fares (origin_crs, destination_crs, fare_class, price_pence, valid_from, valid_to)
         VALUES ($1, $2, $3, $4, $5, $6),
                ($1, $2, $7, $8, $5, $6),
                ($1, $2, $9, $10, $11, $12)",
    )
    .bind("LDS")     // $1
    .bind("MAN")     // $2
    .bind("SDS")     // $3 — cheap valid
    .bind(1000i32)   // $4
    .bind(past)      // $5
    .bind(future)    // $6
    .bind("SOS")     // $7 — expensive valid
    .bind(2500i32)   // $8
    .bind("FOS")     // $9 — expired
    .bind(500i32)    // $10
    .bind(past)      // $11
    .bind(expired)   // $12
    .execute(&pool)
    .await
    .expect("fare insert must succeed");

    let fare = db::static_data::cheapest_fare(&pool, "LDS", "MAN", today)
        .await
        .expect("cheapest_fare must not fail")
        .expect("at least one valid fare must be found");

    assert_eq!(fare.price_pence, 1000, "cheapest valid fare must be 1000p, got {}p", fare.price_pence);
    assert_eq!(fare.fare_class, "SDS", "cheapest fare must have class 'SDS'");
    assert_eq!(fare.origin_crs.trim(), "LDS");
    assert_eq!(fare.destination_crs.trim(), "MAN");
}

// ---------------------------------------------------------------------------
// Test 12: operator_toc_join_and_reference
//
// Verify that:
//   1. services.toc can be set and JOINed to delay_history via services.uid.
//   2. list_operators returns the inserted operator with the correct fields.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn operator_toc_join_and_reference(pool: sqlx::PgPool) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO stations (crs, name) VALUES ('AAA','Alpha'),('BBB','Beta')")
        .execute(&pool).await?;
    sqlx::query(
        "INSERT INTO services (uid, origin_crs, destination_crs, runs_on_days, toc) \
         VALUES ('C12345','AAA','BBB',127,'GW')",
    )
    .execute(&pool).await?;
    sqlx::query(
        "INSERT INTO operators (toc, name, brand_color) VALUES ('GW','Great Western Railway','#0a493e')",
    )
    .execute(&pool).await?;
    sqlx::query(
        "INSERT INTO delay_history (uid, weekday, origin_crs, delay_mins) \
         VALUES ('C12345',0,'AAA',5)",
    )
    .execute(&pool).await?;

    // The JOIN labels the history row with its operator — no toc on delay_history.
    let toc: String = sqlx::query_scalar(
        "SELECT s.toc FROM delay_history d JOIN services s ON s.uid = d.uid LIMIT 1",
    ).fetch_one(&pool).await?;
    assert_eq!(toc, "GW");

    let ops = railpredict::db::operators::list_operators(&pool).await?;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].name, "Great Western Railway");
    assert_eq!(ops[0].brand_color, "#0a493e");
    Ok(())
}

// ---------------------------------------------------------------------------
// Test 11: cheapest_fare excludes fares not yet valid (valid_from in future)
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn cheapest_fare_excludes_future_fares(pool: sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
    )
    .bind("LDS")
    .bind("Leeds")
    .bind("MAN")
    .bind("Manchester Piccadilly")
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let today = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let future_valid_from = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();

    sqlx::query(
        "INSERT INTO fares (origin_crs, destination_crs, fare_class, price_pence, valid_from)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind("LDS")
    .bind("MAN")
    .bind("SDS")
    .bind(999i32)
    .bind(future_valid_from)
    .execute(&pool)
    .await
    .expect("fare insert must succeed");

    let result = db::static_data::cheapest_fare(&pool, "LDS", "MAN", today)
        .await
        .expect("cheapest_fare must not fail");

    assert!(
        result.is_none(),
        "cheapest_fare must not return fares with valid_from in the future"
    );
}

#[sqlx::test(migrations = "../migrations")]
async fn overview_headline_metrics_basic(pool: sqlx::PgPool) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO delay_history (uid, weekday, origin_crs, delay_mins, predicted_delay_mins) VALUES
         ('C00001',0,'AAA',0,2),
         ('C00002',0,'AAA',-1,NULL),
         ('C00003',0,'AAA',10,6)",
    )
    .execute(&pool)
    .await?;

    let m = railpredict::db::overview::headline_metrics(&pool, 24).await?;
    assert_eq!(m.sample_count, 3);
    assert!((m.on_time_pct.unwrap() - 66.6667).abs() < 0.1, "on_time_pct = {:?}", m.on_time_pct);
    assert!((m.avg_delay_mins.unwrap() - 3.0).abs() < 0.001);
    assert!((m.mae_mins.unwrap() - 3.0).abs() < 0.001, "mae = {:?}", m.mae_mins);

    let series = railpredict::db::overview::daily_series(&pool, 24).await?;
    assert_eq!(series.len(), 1, "all rows fall on one day");

    let cov = railpredict::db::overview::coverage_counts(&pool).await?;
    assert_eq!(cov.real_records, 3);
    assert_eq!(cov.predictions_scored, 0, "no finalised predictions seeded in this test");
    Ok(())
}

// ---------------------------------------------------------------------------
// Test 13: operator_league_ranks_by_on_time
//
// Two operators (GW: 100% on time, VT: 0% on time). Verify the league table
// returns both rows ranked GW first, with correct on_time_pct values.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn operator_league_ranks_by_on_time(pool: sqlx::PgPool) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO stations (crs, name) VALUES ('AAA','Alpha'),('BBB','Beta')").execute(&pool).await?;
    sqlx::query("INSERT INTO services (uid, origin_crs, destination_crs, runs_on_days, toc) VALUES
                 ('C00001','AAA','BBB',127,'GW'),
                 ('C00002','AAA','BBB',127,'VT')").execute(&pool).await?;
    sqlx::query("INSERT INTO operators (toc, name, brand_color) VALUES
                 ('GW','Great Western','#0a493e'),
                 ('VT','Avanti','#11354e')").execute(&pool).await?;
    // GW: 2/2 on time. VT: 0/2 on time.
    // Use distinct departure_hour values so (uid, weekday, origin_crs, departure_hour, recorded_at)
    // unique index is satisfied when recorded_at falls at the same instant.
    sqlx::query("INSERT INTO delay_history (uid, weekday, origin_crs, departure_hour, delay_mins) VALUES
                 ('C00001',0,'AAA',9, 0),
                 ('C00001',0,'AAA',10,-2),
                 ('C00002',0,'AAA',9, 9),
                 ('C00002',0,'AAA',10,12)").execute(&pool).await?;

    let rows = railpredict::db::operators::operator_league(&pool, 24, 1, 10).await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].toc, "GW", "GW ranks first (100% on time)");
    assert!((rows[0].on_time_pct.unwrap() - 100.0).abs() < 0.001);
    assert_eq!(rows[1].toc, "VT");
    assert!(rows[1].on_time_pct.unwrap() < 1.0);
    Ok(())
}

// ---------------------------------------------------------------------------
// direct_journeys: the timetable self-join relocated out of frontend/ into db/.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn direct_journeys_finds_through_service(pool: sqlx::PgPool) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO stations (crs, name) VALUES ($1,$2),($3,$4) ON CONFLICT (crs) DO NOTHING")
        .bind("LDS").bind("Leeds").bind("MAN").bind("Manchester Piccadilly")
        .execute(&pool).await?;
    sqlx::query("INSERT INTO services (uid, origin_crs, destination_crs) VALUES ($1,$2,$3) ON CONFLICT (uid) DO NOTHING")
        .bind("C12345").bind("LDS").bind("MAN")
        .execute(&pool).await?;

    let date = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let dep = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let arr = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
    // LDS at call_order 0 precedes MAN at call_order 1 for the same service.
    sqlx::query(
        "INSERT INTO timetable_calls (uid, operating_date, location_crs, call_order, scheduled_departure, public_departure, platform)
         VALUES ($1,$2,'LDS',0,$3,$3,'1'), ($1,$2,'MAN',1,$4,$4,'2')",
    )
    .bind("C12345").bind(date).bind(dep).bind(arr)
    .execute(&pool).await?;

    // Forward direction finds the through service.
    let rows = db::static_data::direct_journeys(&pool, "LDS", "MAN", date, None).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0.trim(), "C12345");
    assert_eq!(rows[0].1, dep);

    // Reverse direction has no qualifying journey (no MAN call before an LDS call).
    let none = db::static_data::direct_journeys(&pool, "MAN", "LDS", date, None).await?;
    assert!(none.is_empty());

    // The limit param caps the result set.
    let limited = db::static_data::direct_journeys(&pool, "LDS", "MAN", date, Some(0)).await?;
    assert!(limited.is_empty(), "LIMIT 0 should return nothing");
    Ok(())
}

// ---------------------------------------------------------------------------
// cancellations: write + derived pattern fields + recent count.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn cancellations_record_and_count(pool: sqlx::PgPool) -> sqlx::Result<()> {
    use chrono::{TimeZone, Utc};
    // 2024-04-17 is a Wednesday → weekday 2 (0=Mon); 08:30 → hour 8.
    let sched = Utc.with_ymd_and_hms(2024, 4, 17, 8, 30, 0).unwrap();
    db::cancellations::record_cancellation(&pool, "C12345", "LDS", sched).await?;
    db::cancellations::record_cancellation(&pool, "C99999", "MAN", sched).await?;

    assert_eq!(db::cancellations::count_recent(&pool, 24).await?, 2);

    let (wd, hr): (i16, i16) =
        sqlx::query_as("SELECT weekday, departure_hour FROM cancellations WHERE uid = 'C12345'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(wd, 2, "Wednesday");
    assert_eq!(hr, 8);
    Ok(())
}

// ---------------------------------------------------------------------------
// journeys: header + per-stop calls round-trip + idempotent re-insert.
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn insert_journey_persists_header_and_calls(pool: sqlx::PgPool) -> sqlx::Result<()> {
    use chrono::{Duration, TimeZone, Utc};
    use railpredict::db::journeys::{insert_journey, JourneyCallRecord, JourneyRecord};

    let sched = Utc.with_ymd_and_hms(2026, 6, 5, 8, 0, 0).unwrap();
    let rid = "202606050123456";
    let rec = JourneyRecord {
        rid: rid.to_string(),
        uid: "C12345".into(),
        ssd: NaiveDate::from_ymd_opt(2026, 6, 5).unwrap(),
        weekday: 4,
        departure_hour: 8,
        toc: Some("GW".into()),
        train_category: Some("OO".into()),
        origin_tpl: "PADTON".into(),
        destination_tpl: Some("BRISTM".into()),
        scheduled_departure: sched,
        actual_departure: Some(sched + Duration::minutes(2)),
        origin_delay_mins: Some(2),
        arrival_delay_mins: Some(6),
        late_reason_code: Some(168),
        cancel_reason_code: None,
        reason_tiploc: Some("READING".into()),
        reason_class: 1,
        was_cancelled: false,
        partial_cancel: false,
        n_calls: 2,
        max_delay_mins: Some(12),
        min_delay_mins: Some(2),
        recovered_mins: Some(6),
        origin_platform: Some("9".into()),
        platform_confirmed: Some(true),
        wind_mph: Some(14.0),
    };
    let calls = vec![
        JourneyCallRecord {
            seq: 0,
            tpl: "PADTON".into(),
            sched_arr: None,
            actual_arr: None,
            arr_delay_mins: None,
            sched_dep: Some(sched),
            actual_dep: Some(sched + Duration::minutes(2)),
            dep_delay_mins: Some(2),
            platform: Some("9".into()),
            plat_confirmed: Some(true),
            is_cancelled: false,
            dwell_secs: None,
        },
        JourneyCallRecord {
            seq: 1,
            tpl: "BRISTM".into(),
            sched_arr: Some(sched + Duration::minutes(90)),
            actual_arr: Some(sched + Duration::minutes(96)),
            arr_delay_mins: Some(6),
            sched_dep: None,
            actual_dep: None,
            dep_delay_mins: None,
            platform: None,
            plat_confirmed: None,
            is_cancelled: false,
            dwell_secs: None,
        },
    ];

    insert_journey(&pool, &rec, &calls).await?;
    // Idempotent: a re-finalisation must not duplicate or error.
    insert_journey(&pool, &rec, &calls).await?;

    let n_journeys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journeys WHERE rid = $1")
        .bind(rid)
        .fetch_one(&pool)
        .await?;
    assert_eq!(n_journeys, 1, "header inserted once");

    let n_calls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM journey_calls WHERE rid = $1")
        .bind(rid)
        .fetch_one(&pool)
        .await?;
    assert_eq!(n_calls, 2, "both calls inserted, no duplicates");

    let (arr, toc): (Option<i32>, Option<String>) =
        sqlx::query_as("SELECT arrival_delay_mins, toc FROM journeys WHERE rid = $1")
            .bind(rid)
            .fetch_one(&pool)
            .await?;
    assert_eq!(arr, Some(6));
    assert_eq!(toc.as_deref().map(str::trim), Some("GW"));

    assert_eq!(db::journeys::count_recent(&pool, 24).await?, 1);
    Ok(())
}

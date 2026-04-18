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
//! These tests import from `railpredict::db` and `railpredict::prediction::types`.
//! If any types used here are not `pub`, the compiler will report an error.
//! All types accessed (`HistoricalStore`, `ServicePattern`, `DelayRecord`) are `pub`
//! in `src/prediction/types.rs`.

use std::sync::Arc;

use chrono::{NaiveDate, NaiveTime, Utc};

use railpredict::db;
use railpredict::prediction::types::{DelayRecord, HistoricalStore, ServicePattern};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `ServicePattern` for use in tests.
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

    // HistoricalStore does not expose `is_empty()`. Use `all_records()` instead
    // (returns Vec — empty vec means no records loaded).
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
    // Two stations must exist before we can insert delay_history rows that
    // reference them via origin_crs. Insert them first.
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
        "MAN",
        "Manchester Piccadilly",
    )
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

    // Use slightly different timestamps to avoid the unique constraint on
    // (uid, weekday, origin_crs, departure_hour, recorded_at).
    let base_time = Utc::now();
    let t1 = base_time - chrono::Duration::seconds(2);
    let t2 = base_time - chrono::Duration::seconds(1);
    let t3 = base_time;

    store.insert(p1.clone(), DelayRecord { delay_mins: 5, recorded_at: t1 });
    store.insert(p1.clone(), DelayRecord { delay_mins: 10, recorded_at: t2 });
    store.insert(p2.clone(), DelayRecord { delay_mins: 0, recorded_at: t3 });

    db::history::flush_history(&pool, &store)
        .await
        .expect("flush_history must not fail");

    // Reload from the database.
    let loaded = db::history::load_history(&pool)
        .await
        .expect("load_history must not fail after flush");

    // p1 should have 2 samples.
    let samples_p1 = loaded.get_samples(&p1).expect("p1 should have samples after reload");
    assert_eq!(samples_p1.len(), 2, "p1 must have 2 samples after roundtrip");
    assert!(samples_p1.contains(&5), "p1 samples must contain delay_mins=5");
    assert!(samples_p1.contains(&10), "p1 samples must contain delay_mins=10");

    // p2 should have 1 sample.
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
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2) ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
    )
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let store = Arc::new(HistoricalStore::new());
    let record = DelayRecord { delay_mins: 3, recorded_at: Utc::now() };
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
    sqlx::query!(
        "INSERT INTO stations (crs, name, nlc, lat, lon, is_active)
         VALUES ($1, $2, $3, $4, $5, $6)",
        "KGX",
        "London Kings Cross",
        "5592",
        51.5308_f64,
        -0.1238_f64,
        true,
    )
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

    // Coordinates are stored as f64; allow for minor floating-point tolerance.
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
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2) ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
    )
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
    // Stations required by FK constraints.
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
        "MAN",
        "Manchester Piccadilly",
    )
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    // Service required by FK in timetable_calls.
    sqlx::query!(
        "INSERT INTO services (uid, origin_crs, destination_crs)
         VALUES ($1, $2, $3)
         ON CONFLICT (uid) DO NOTHING",
        "C12345",
        "LDS",
        "MAN",
    )
    .execute(&pool)
    .await
    .expect("service seed must succeed");

    let operating_date = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let dep_0900 = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let dep_0930 = NaiveTime::from_hms_opt(9, 30, 0).unwrap();

    // Two calls from Leeds on the same date.
    sqlx::query!(
        "INSERT INTO timetable_calls
             (uid, operating_date, location_crs, call_order, scheduled_departure, public_departure, platform)
         VALUES ($1, $2, $3, $4, $5, $6, $7),
                ($1, $2, $3, $8, $9, $9, $10)",
        "C12345",        // uid
        operating_date,  // operating_date
        "LDS",           // location_crs
        0i16,            // call_order (first call)
        dep_0900,        // scheduled_departure
        dep_0900,        // public_departure
        "1",             // platform
        1i16,            // call_order (second call)
        dep_0930,        // scheduled_departure / public_departure
        "2A",            // platform
    )
    .execute(&pool)
    .await
    .expect("timetable_calls insert must succeed");

    let calls = db::static_data::departures_from(&pool, "LDS", operating_date)
        .await
        .expect("departures_from must not fail");

    assert_eq!(calls.len(), 2, "Expected exactly 2 calls, got {}", calls.len());

    // Verify ordering: 09:00 first, 09:30 second.
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
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
        "MAN",
        "Manchester Piccadilly",
    )
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    sqlx::query!(
        "INSERT INTO services (uid, origin_crs, destination_crs)
         VALUES ($1, $2, $3) ON CONFLICT (uid) DO NOTHING",
        "C12345",
        "LDS",
        "MAN",
    )
    .execute(&pool)
    .await
    .expect("service seed must succeed");

    let target_date = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let other_date = NaiveDate::from_ymd_opt(2024, 4, 18).unwrap();
    let dep_time = NaiveTime::from_hms_opt(10, 0, 0).unwrap();

    sqlx::query!(
        "INSERT INTO timetable_calls
             (uid, operating_date, location_crs, call_order, scheduled_departure)
         VALUES ($1, $2, $3, $4, $5)",
        "C12345",
        other_date, // inserted for a DIFFERENT date
        "LDS",
        0i16,
        dep_time,
    )
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
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
        "MAN",
        "Manchester Piccadilly",
    )
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
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
        "MAN",
        "Manchester Piccadilly",
    )
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let today = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let past = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    let future = NaiveDate::from_ymd_opt(2030, 12, 31).unwrap();

    // Insert fares:
    //  - 1000p (valid) — the cheapest valid fare
    //  - 2500p (valid) — more expensive valid fare
    //  - 500p  (expired, valid_to in the past) — must be excluded
    sqlx::query!(
        "INSERT INTO fares (origin_crs, destination_crs, fare_class, price_pence, valid_from, valid_to)
         VALUES ($1, $2, $3, $4, $5, $6),
                ($1, $2, $7, $8, $5, $6),
                ($1, $2, $9, $10, $11, $12)",
        "LDS",              // origin_crs
        "MAN",              // destination_crs
        "SDS",              // fare_class (cheap valid)
        1000i32,            // price_pence
        past,               // valid_from
        future,             // valid_to
        "SOS",              // fare_class (expensive valid)
        2500i32,            // price_pence
        "FOS",              // fare_class (expired)
        500i32,             // price_pence (cheapest but expired)
        past,               // valid_from
        NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(), // valid_to in the past
    )
    .execute(&pool)
    .await
    .expect("fare insert must succeed");

    let fare = db::static_data::cheapest_fare(&pool, "LDS", "MAN", today)
        .await
        .expect("cheapest_fare must not fail")
        .expect("at least one valid fare must be found");

    assert_eq!(
        fare.price_pence, 1000,
        "cheapest valid fare must be 1000p, got {}p",
        fare.price_pence
    );
    assert_eq!(fare.fare_class, "SDS", "cheapest fare must have class 'SDS'");
    assert_eq!(fare.origin_crs.trim(), "LDS");
    assert_eq!(fare.destination_crs.trim(), "MAN");
}

// ---------------------------------------------------------------------------
// Test 11: cheapest_fare excludes fares not yet valid (valid_from in future)
// ---------------------------------------------------------------------------
#[sqlx::test(migrations = "../migrations")]
async fn cheapest_fare_excludes_future_fares(pool: sqlx::PgPool) {
    sqlx::query!(
        "INSERT INTO stations (crs, name) VALUES ($1, $2), ($3, $4)
         ON CONFLICT (crs) DO NOTHING",
        "LDS",
        "Leeds",
        "MAN",
        "Manchester Piccadilly",
    )
    .execute(&pool)
    .await
    .expect("station seed must succeed");

    let today = NaiveDate::from_ymd_opt(2024, 4, 17).unwrap();
    let future_valid_from = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();

    sqlx::query!(
        "INSERT INTO fares (origin_crs, destination_crs, fare_class, price_pence, valid_from)
         VALUES ($1, $2, $3, $4, $5)",
        "LDS",
        "MAN",
        "SDS",
        999i32,
        future_valid_from, // not yet valid on `today`
    )
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

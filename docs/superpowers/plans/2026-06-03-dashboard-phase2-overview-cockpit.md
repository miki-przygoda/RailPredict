# Dashboard Phase 2 — Overview Cockpit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild `/` into a live, time-rangeable "Signal Terminal" metrics cockpit — a KPI strip with real figures + sparklines, a live network-state panel from the registry, a mini operator league, all re-scopable by the global time-range picker.

**Architecture:** New read-only DB aggregate helpers (`db/overview.rs`) and an operator-league query (`db/operators.rs`) feed the KPI strip and league; a new async `TrainRegistry::network_summary` reads live `TrainStatus` snapshots for the network panel. The dashboard handler gains a `?range=` query param and returns the full page for normal loads or just the cockpit fragment for htmx swaps (so the time-range picker re-scopes everything in place). All chart rendering uses the existing `frontend/charts.rs` SVG helpers — no new JS.

**Tech Stack:** Rust, axum, sqlx (Postgres), maud, htmx, inline SVG.

**Spec:** `docs/superpowers/specs/2026-06-03-dashboard-overhaul-design.md` (§5 `/`, §4)
**Builds on:** Phase 0 (charts.rs, time_range_picker, Signal Terminal tokens), Phase 1 (`services.toc`, `operators`, `list_operators`).

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `RailPredict/src/db/overview.rs` | `headline_metrics` + `daily_series` aggregate reads | Create |
| `RailPredict/src/db/operators.rs` | add `OperatorLeagueRow` + `operator_league` | Modify |
| `RailPredict/src/db/mod.rs` | register `overview` | Modify |
| `RailPredict/src/cache/train_registry.rs` | `NetworkSummary`/`LiveDelay` + async `network_summary` | Modify |
| `RailPredict/src/frontend/dashboard.rs` | rebuilt cockpit handler + render | Rewrite |
| `RailPredict/static/style.css` | cockpit panel CSS (network panel, league, coverage chips) | Modify |
| `RailPredict/tests/db_integration.rs` | sqlx::tests for the new queries | Modify |
| `CLAUDE.md`/`README.md`/`TODO.md`/`CHANGELOG.md`/`RailPredict/Cargo.toml` | version → 1.12.7 | Modify |

**Phase-2 scope (YAGNI):** the overview cockpit only. The full `/operators` league + drill-down is Phase 3; here the mini-league is a top-8 read-only widget. The predictions explorer is Phase 4. Range values are the four the picker already emits (`24h`/`7d`/`30d`/`all`).

**Time-range mapping:** `24h→24`, `7d→168`, `30d→720`, `all→876000` (hours; ~100y ≈ unbounded). All aggregate queries bound on `recorded_at > NOW() - $1::INT * INTERVAL '1 hour'` (mirrors `db/predictions.rs::accuracy_summary`).

---

## Task 1: `db/overview.rs` — headline metrics + daily series (TDD via sqlx::test)

**Files:**
- Create: `RailPredict/src/db/overview.rs`
- Modify: `RailPredict/src/db/mod.rs`
- Test: `RailPredict/tests/db_integration.rs`

- [ ] **Step 1: Create `db/overview.rs`**
```rust
//! Read-only aggregate metrics for the overview cockpit (`/`).
//!
//! All queries bound on a rolling window of `hours` and apply the same delay
//! sanity filter (`delay_mins BETWEEN -120 AND 600`) used elsewhere, so the
//! severe-delay tail of the Darwin feed can't distort the headline figures.

use sqlx::FromRow;

use super::Db;

/// Headline figures for the KPI strip over a rolling window.
#[derive(Debug, Clone, Default, FromRow)]
pub struct HeadlineMetrics {
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub mae_mins: Option<f64>,
    pub sample_count: i64,
}

/// One day's aggregates, for sparklines.
#[derive(Debug, Clone, FromRow)]
pub struct DailyPoint {
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub mae_mins: Option<f64>,
}

/// Headline metrics over the last `hours` hours.
pub async fn headline_metrics(db: &Db, hours: i32) -> sqlx::Result<HeadlineMetrics> {
    sqlx::query_as::<_, HeadlineMetrics>(
        r#"
        SELECT
            (AVG(CASE WHEN delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8  AS on_time_pct,
            AVG(delay_mins::float8)                                              AS avg_delay_mins,
            (AVG(ABS(predicted_delay_mins - delay_mins))
                FILTER (WHERE predicted_delay_mins IS NOT NULL))::float8         AS mae_mins,
            COUNT(*)                                                            AS sample_count
        FROM delay_history
        WHERE recorded_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND delay_mins BETWEEN -120 AND 600
        "#,
    )
    .bind(hours)
    .fetch_one(db)
    .await
}

/// Per-day aggregates over the last `hours` hours, ordered oldest → newest.
pub async fn daily_series(db: &Db, hours: i32) -> sqlx::Result<Vec<DailyPoint>> {
    sqlx::query_as::<_, DailyPoint>(
        r#"
        SELECT
            (AVG(CASE WHEN delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8  AS on_time_pct,
            AVG(delay_mins::float8)                                              AS avg_delay_mins,
            (AVG(ABS(predicted_delay_mins - delay_mins))
                FILTER (WHERE predicted_delay_mins IS NOT NULL))::float8         AS mae_mins
        FROM delay_history
        WHERE recorded_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND delay_mins BETWEEN -120 AND 600
        GROUP BY date_trunc('day', recorded_at AT TIME ZONE 'Europe/London')
        ORDER BY date_trunc('day', recorded_at AT TIME ZONE 'Europe/London')
        "#,
    )
    .bind(hours)
    .fetch_all(db)
    .await
}

/// Range-independent data-coverage totals shown in the cockpit footer strip.
#[derive(Debug, Clone, Default, FromRow)]
pub struct CoverageCounts {
    pub stations: i64,
    pub real_records: i64,
    pub synthetic_records: i64,
}

/// Total station and delay-history row counts (all-time, not windowed).
/// Exact counts, mirroring the previous dashboard behaviour.
pub async fn coverage_counts(db: &Db) -> sqlx::Result<CoverageCounts> {
    sqlx::query_as::<_, CoverageCounts>(
        r#"
        SELECT
            (SELECT COUNT(*) FROM stations)                  AS stations,
            (SELECT COUNT(*) FROM delay_history)             AS real_records,
            (SELECT COUNT(*) FROM delay_history_synthetic)   AS synthetic_records
        "#,
    )
    .fetch_one(db)
    .await
}
```

- [ ] **Step 2: Register in `db/mod.rs`** (alongside the other `pub mod` lines):
```rust
pub mod overview;
```

- [ ] **Step 3: Build**
```bash
cd RailPredict && cargo build 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 4: Add a sqlx::test to `tests/db_integration.rs`**

Match the existing test style (the `railpredict::` crate path + `sqlx::PgPool` param confirmed by the existing tests). Insert a couple of `delay_history` rows and assert the aggregates:
```rust
#[sqlx::test]
async fn overview_headline_metrics_basic(pool: sqlx::PgPool) -> sqlx::Result<()> {
    // 3 rows: two on-time (delay <= 0), one delayed; one has a prediction.
    sqlx::query(
        "INSERT INTO delay_history (uid, weekday, origin_crs, delay_mins, predicted_delay_mins) VALUES
         ('C00001',0,'AAA',0,2),
         ('C00002',0,'AAA',-1,NULL),
         ('C00003',0,'AAA',10,6)",
    ).execute(&pool).await?;

    let m = railpredict::db::overview::headline_metrics(&pool, 24).await?;
    assert_eq!(m.sample_count, 3);
    // 2 of 3 on time → 66.6…%
    assert!((m.on_time_pct.unwrap() - 66.6667).abs() < 0.1, "on_time_pct = {:?}", m.on_time_pct);
    // avg delay = (0 + -1 + 10) / 3 = 3.0
    assert!((m.avg_delay_mins.unwrap() - 3.0).abs() < 0.001);
    // MAE over the 2 predicted rows: |2-0|=2, |6-10|=4 → 3.0
    assert!((m.mae_mins.unwrap() - 3.0).abs() < 0.001, "mae = {:?}", m.mae_mins);

    let series = railpredict::db::overview::daily_series(&pool, 24).await?;
    assert_eq!(series.len(), 1, "all rows fall on one day");

    let cov = railpredict::db::overview::coverage_counts(&pool).await?;
    assert_eq!(cov.real_records, 3);
    assert_eq!(cov.synthetic_records, 0);
    Ok(())
}
```

- [ ] **Step 5: Run the test (DB available in this repo's test env)**
```bash
cd RailPredict && cargo test --test db_integration overview_headline_metrics_basic 2>&1 | tail -12
```
Expected: PASS. If no Postgres is reachable in your sandbox, confirm `cargo test --test db_integration --no-run` compiles and report DONE_WITH_CONCERNS (do not BLOCK on a missing DB).

- [ ] **Step 6: Commit**
```bash
git add RailPredict/src/db/overview.rs RailPredict/src/db/mod.rs RailPredict/tests/db_integration.rs
git commit -m "feat(db): overview headline_metrics + daily_series aggregates"
```

---

## Task 2: `operator_league` query (TDD via sqlx::test)

**Files:**
- Modify: `RailPredict/src/db/operators.rs`
- Test: `RailPredict/tests/db_integration.rs`

- [ ] **Step 1: Add the row type + query to `db/operators.rs`** (below `list_operators`)
```rust
/// One operator's punctuality aggregates over a rolling window — a league row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OperatorLeagueRow {
    pub toc: String,
    pub name: String,
    pub brand_color: String,
    pub on_time_pct: Option<f64>,
    pub avg_delay_mins: Option<f64>,
    pub sample_count: i64,
}

/// Operator league table over the last `hours` hours: delay_history JOINed to
/// services (by uid) and operators (by toc), grouped by operator, ranked by
/// on-time %. Only operators with at least `min_samples` rows are included.
/// Returns empty until `services.toc` is populated (run the GTFS ingest).
pub async fn operator_league(
    db: &Db,
    hours: i32,
    min_samples: i64,
    limit: i64,
) -> sqlx::Result<Vec<OperatorLeagueRow>> {
    sqlx::query_as::<_, OperatorLeagueRow>(
        r#"
        SELECT
            s.toc                                                               AS toc,
            COALESCE(o.name, s.toc)                                             AS name,
            COALESCE(o.brand_color, '#9aa7b4')                                  AS brand_color,
            (AVG(CASE WHEN d.delay_mins <= 0 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
            AVG(d.delay_mins::float8)                                           AS avg_delay_mins,
            COUNT(*)                                                            AS sample_count
        FROM delay_history d
        JOIN services s ON s.uid = d.uid
        LEFT JOIN operators o ON o.toc = s.toc
        WHERE d.recorded_at > NOW() - $1::INT * INTERVAL '1 hour'
          AND d.delay_mins BETWEEN -120 AND 600
          AND s.toc IS NOT NULL
        GROUP BY s.toc, o.name, o.brand_color
        HAVING COUNT(*) >= $2
        ORDER BY on_time_pct DESC NULLS LAST
        LIMIT $3
        "#,
    )
    .bind(hours)
    .bind(min_samples)
    .bind(limit)
    .fetch_all(db)
    .await
}
```

- [ ] **Step 2: Build**
```bash
cd RailPredict && cargo build 2>&1 | tail -3
```

- [ ] **Step 3: Add a sqlx::test to `tests/db_integration.rs`**
```rust
#[sqlx::test]
async fn operator_league_ranks_by_on_time(pool: sqlx::PgPool) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO stations (crs, name) VALUES ('AAA','Alpha'),('BBB','Beta')").execute(&pool).await?;
    sqlx::query("INSERT INTO services (uid, origin_crs, destination_crs, runs_on_days, toc) VALUES
                 ('C00001','AAA','BBB',127,'GW'),
                 ('C00002','AAA','BBB',127,'VT')").execute(&pool).await?;
    sqlx::query("INSERT INTO operators (toc, name, brand_color) VALUES
                 ('GW','Great Western','#0a493e'),
                 ('VT','Avanti','#11354e')").execute(&pool).await?;
    // GW: 2/2 on time. VT: 0/2 on time.
    sqlx::query("INSERT INTO delay_history (uid, weekday, origin_crs, delay_mins) VALUES
                 ('C00001',0,'AAA',0),('C00001',0,'AAA',-2),
                 ('C00002',0,'AAA',9),('C00002',0,'AAA',12)").execute(&pool).await?;

    let rows = railpredict::db::operators::operator_league(&pool, 24, 1, 10).await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].toc, "GW", "GW ranks first (100% on time)");
    assert!((rows[0].on_time_pct.unwrap() - 100.0).abs() < 0.001);
    assert_eq!(rows[1].toc, "VT");
    assert!(rows[1].on_time_pct.unwrap() < 1.0);
    Ok(())
}
```

- [ ] **Step 4: Run + commit**
```bash
cd RailPredict && cargo test --test db_integration operator_league_ranks_by_on_time 2>&1 | tail -12
```
Expected: PASS (or DONE_WITH_CONCERNS + `--no-run` if no DB). Then:
```bash
git add RailPredict/src/db/operators.rs RailPredict/tests/db_integration.rs
git commit -m "feat(db): operator_league punctuality query"
```

---

## Task 3: `TrainRegistry::network_summary` — live network panel data (TDD)

**Files:**
- Modify: `RailPredict/src/cache/train_registry.rs` (types + async method + unit test)

- [ ] **Step 1: Read the existing registry tests to learn how `TrainStatus` is constructed**
```bash
cd RailPredict && sed -n '430,520p' src/cache/train_registry.rs
```
Note the helper/constructor the tests use to build a `TrainStatus` and `upsert` it (e.g. a `TrainStatus::new(...)` or a local `mk_status(...)` helper). You will mirror it in the new test.

- [ ] **Step 2: Add the summary types near the top of `train_registry.rs` (after the existing `use` lines)**
```rust
/// A single live delayed train, for the cockpit "worst right now" list.
#[derive(Debug, Clone)]
pub struct LiveDelay {
    pub rid: String,
    pub origin_crs: Option<String>,
    pub destination_crs: Option<String>,
    pub delay_mins: i32,
}

/// Live network state derived from the registry snapshot.
#[derive(Debug, Clone, Default)]
pub struct NetworkSummary {
    pub tracked: usize,
    pub on_time: usize,
    pub delayed: usize,
    pub cancelled: usize,
    pub worst: Vec<LiveDelay>,
}
```

- [ ] **Step 3: Add a failing test (inside the `#[cfg(test)] mod tests` block)**

Use the SAME `TrainStatus` construction the existing tests use (from Step 1). Build three trains — one on time, one +12, one cancelled — upsert, and assert. Template (adapt the status constructor to the real helper name/signature):
```rust
    #[tokio::test]
    async fn network_summary_counts_and_ranks() {
        let reg = TrainRegistry::new();
        // on-time train
        reg.upsert(/* TrainId */, /* status with delay 0, not cancelled */);
        // delayed +12
        reg.upsert(/* TrainId */, /* status with reported delay 12 */);
        // cancelled
        reg.upsert(/* TrainId */, /* status is_cancelled = Some(true) */);

        let s = reg.network_summary(5).await;
        assert_eq!(s.tracked, 3);
        assert_eq!(s.cancelled, 1);
        assert_eq!(s.delayed, 1);
        assert_eq!(s.on_time, 1);
        assert_eq!(s.worst.len(), 1);
        assert_eq!(s.worst[0].delay_mins, 12);
    }
```
> Build the `TrainStatus` values via whatever constructor the existing registry tests use; set the reported delay so `best_delay_mins()` returns the intended value, and `is_cancelled.value = Some(true)` for the cancelled one. If the existing tests use a helper like `sample_status(rid)`, extend/reuse it.

- [ ] **Step 4: Run, confirm it fails to compile (`network_summary` undefined)**
```bash
cd RailPredict && cargo test --lib cache::train_registry::tests::network_summary 2>&1 | tail -15
```

- [ ] **Step 5: Implement `network_summary` (in `impl TrainRegistry`)**
```rust
    /// Summarise live network state from the current registry snapshot.
    /// `worst_n` caps the returned worst-delays list. Acquires a read lock per
    /// train (consistent with `departure_snapshot`); cancelled trains are counted
    /// as cancelled and excluded from the on-time/delayed tallies.
    pub async fn network_summary(&self, worst_n: usize) -> NetworkSummary {
        let mut s = NetworkSummary::default();
        let mut delays: Vec<LiveDelay> = Vec::new();
        for arc in self.snapshot_all() {
            let status = arc.read().await;
            s.tracked += 1;
            if status.is_cancelled.value == Some(true) {
                s.cancelled += 1;
                continue;
            }
            match status.best_delay_mins() {
                Some(d) if d > 0 => {
                    s.delayed += 1;
                    delays.push(LiveDelay {
                        rid: status.id.as_str().to_string(),
                        origin_crs: status.origin_crs.clone(),
                        destination_crs: status.destination_crs.clone(),
                        delay_mins: d,
                    });
                }
                _ => s.on_time += 1,
            }
        }
        delays.sort_by(|a, b| b.delay_mins.cmp(&a.delay_mins));
        delays.truncate(worst_n);
        s.worst = delays;
        s
    }
```

- [ ] **Step 6: Run the test, confirm PASS; clippy**
```bash
cd RailPredict && cargo test --lib cache::train_registry 2>&1 | tail -10
cargo clippy --lib 2>&1 | grep "train_registry.rs" || echo "no train_registry.rs warnings"
```

- [ ] **Step 7: Commit**
```bash
git add RailPredict/src/cache/train_registry.rs
git commit -m "feat(cache): TrainRegistry::network_summary for live cockpit panel"
```

---

## Task 4: Rebuild the dashboard cockpit (handler + render)

**Files:**
- Rewrite: `RailPredict/src/frontend/dashboard.rs`

Depends on Tasks 1–3 (uses `overview::*`, `operators::operator_league`, `registry.network_summary`).

- [ ] **Step 1: Replace the handler + imports**

At the top of `dashboard.rs`, set imports:
```rust
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use maud::{Markup, html};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::{operators, overview};
use crate::frontend::charts::{self, Polarity};
use crate::frontend::components;

use super::layout::{base, NavPage};
```

Replace `dashboard_page` and add range helpers:
```rust
#[derive(Debug, Deserialize)]
pub struct DashParams {
    #[serde(default)]
    pub range: Option<String>,
}

/// Normalise an incoming range string to one of the four supported values.
fn normalize_range(raw: Option<&str>) -> &'static str {
    match raw {
        Some("24h") => "24h",
        Some("30d") => "30d",
        Some("all") => "all",
        _ => "7d", // default
    }
}

fn range_to_hours(range: &str) -> i32 {
    match range {
        "24h" => 24,
        "30d" => 720,
        "all" => 876_000, // ~100 years ≈ unbounded
        _ => 168,         // 7d
    }
}

pub async fn dashboard_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<DashParams>,
) -> Markup {
    let range = normalize_range(params.range.as_deref());
    let hours = range_to_hours(range);

    let db_ok = sqlx::query("SELECT 1").execute(&state.db).await.is_ok();

    let (headline, series, league, coverage) = tokio::join!(
        overview::headline_metrics(&state.db, hours),
        overview::daily_series(&state.db, hours),
        operators::operator_league(&state.db, hours, 20, 8),
        overview::coverage_counts(&state.db),
    );
    let net = state.registry.network_summary(6).await;

    let body = render_cockpit(
        range,
        db_ok,
        headline.unwrap_or_default(),
        series.unwrap_or_default(),
        league.unwrap_or_default(),
        coverage.unwrap_or_default(),
        net,
    );

    // htmx range swaps target <main> and want only the cockpit fragment.
    if headers.contains_key("hx-request") {
        body
    } else {
        base("Dashboard", NavPage::Dashboard, body)
    }
}
```

- [ ] **Step 2: Add metric-series helpers + the render function**

Add helpers to turn the daily series into sparkline vectors and a trend delta:
```rust
/// Extract a metric column from the series as an f64 vec (dropping NULL days).
fn col(series: &[overview::DailyPoint], f: impl Fn(&overview::DailyPoint) -> Option<f64>) -> Vec<f64> {
    series.iter().filter_map(|p| f(p)).collect()
}

/// Delta between the last and first non-null points of a series (0.0 if <2 points).
fn delta(vals: &[f64]) -> Option<f64> {
    match (vals.first(), vals.last()) {
        (Some(a), Some(b)) if vals.len() >= 2 => Some(b - a),
        _ => None,
    }
}

/// Format an optional metric value, or an em dash when absent.
fn fmt_opt(v: Option<f64>, prec: usize) -> String {
    match v {
        Some(x) => format!("{x:.*}", prec),
        None => "—".to_string(),
    }
}
```

Now the render function. It composes: KPI strip (live + sparkline + polarity), the network-state panel, the mini operator league (or empty state), and a slim hero/quick-access footer with inline-SVG icons:
```rust
fn render_cockpit(
    range: &str,
    db_ok: bool,
    headline: overview::HeadlineMetrics,
    series: Vec<overview::DailyPoint>,
    league: Vec<operators::OperatorLeagueRow>,
    coverage: overview::CoverageCounts,
    net: crate::cache::train_registry::NetworkSummary,
) -> Markup {
    let ontime_spark = col(&series, |p| p.on_time_pct);
    let delay_spark = col(&series, |p| p.avg_delay_mins);
    let mae_spark = col(&series, |p| p.mae_mins);

    html! {
        div .dashboard {
            div .dash-header {
                div {
                    h1 .dash-title { "Network Overview" }
                    p .dash-sub { "Live UK rail punctuality & prediction accuracy" }
                }
                (components::time_range_picker("/", range))
            }

            // ── KPI strip ────────────────────────────────────────────────
            div .kpi-strip {
                (charts::kpi_card(
                    "On-time", &fmt_opt(headline.on_time_pct, 1), Some("%"),
                    delta(&ontime_spark).map(|d| (d, Polarity::HigherIsBetter)),
                    Some(&ontime_spark)))
                (charts::kpi_card(
                    "Avg delay", &fmt_opt(headline.avg_delay_mins, 1), Some("min"),
                    delta(&delay_spark).map(|d| (d, Polarity::LowerIsBetter)),
                    Some(&delay_spark)))
                (charts::kpi_card(
                    "Prediction MAE", &fmt_opt(headline.mae_mins, 2), Some("min"),
                    delta(&mae_spark).map(|d| (d, Polarity::LowerIsBetter)),
                    Some(&mae_spark)))
                (charts::kpi_card(
                    "Trains tracked", &net.tracked.to_string(), None, None, None))
            }

            div .cockpit-grid {
                // ── Live network state ───────────────────────────────────
                section .panel {
                    div .panel-head {
                        h2 { "Live network" }
                        span .panel-meta {
                            @if db_ok { "Darwin feed" } @else { "DB offline" }
                        }
                    }
                    div .net-counts {
                        (net_stat("Tracked", net.tracked, "net-neutral"))
                        (net_stat("On time", net.on_time, "net-ok"))
                        (net_stat("Delayed", net.delayed, "net-warn"))
                        (net_stat("Cancelled", net.cancelled, "net-bad"))
                    }
                    @if net.worst.is_empty() {
                        p .panel-empty { "No delayed trains right now." }
                    } @else {
                        table .mini-table {
                            thead { tr { th { "Service" } th .num { "Delay" } } }
                            tbody {
                                @for w in &net.worst {
                                    tr {
                                        td {
                                            code { (w.origin_crs.as_deref().unwrap_or("???")) }
                                            span .arrow { "→" }
                                            code { (w.destination_crs.as_deref().unwrap_or("???")) }
                                        }
                                        td .num { span .delay-bad { "+" (w.delay_mins) "m" } }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Operator league (mini) ───────────────────────────────
                section .panel {
                    div .panel-head {
                        h2 { "Operator league" }
                        a .panel-link href="/operators" { "All operators →" }
                    }
                    @if league.is_empty() {
                        p .panel-empty {
                            "No operator data yet. Run the GTFS ingest to populate operators."
                        }
                    } @else {
                        table .mini-table .league-table {
                            thead { tr { th { "Operator" } th .num { "On-time" } th .num { "Avg" } } }
                            tbody {
                                @for row in &league {
                                    tr {
                                        td {
                                            span .op-chip style=(format!("background:{}", row.brand_color)) {}
                                            (row.name)
                                        }
                                        td .num { (fmt_opt(row.on_time_pct, 0)) "%" }
                                        td .num { (fmt_opt(row.avg_delay_mins, 1)) }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Quick access ─────────────────────────────────────────────
            section .dash-section {
                p .dash-section-label { "Explore" }
                div .dash-nav-grid {
                    (nav_card("/search", ICON_BOARD, "Departure Board", "Live departures from any UK station."))
                    (nav_card("/predictions", ICON_CHART, "Prediction analytics", "Accuracy, calibration & biggest errors."))
                    (nav_card("/operators", ICON_TROPHY, "Operators", "Per-operator punctuality league & drill-down."))
                    (nav_card("/demo", ICON_GEAR, "Dev Console", "Ingest data, probe the registry, simulate checkout."))
                }
            }

            // ── Data coverage (range-independent totals) ──────────────────
            div .coverage-strip {
                (coverage_chip("Stations", fmt_big(coverage.stations)))
                (coverage_chip("Real delay records", fmt_big(coverage.real_records)))
                (coverage_chip("Synthetic records", fmt_big(coverage.synthetic_records)))
            }
        }
    }
}

fn coverage_chip(label: &str, value: String) -> Markup {
    html! {
        div .cov-chip {
            span .cov-value { (value) }
            span .cov-label { (label) }
        }
    }
}

/// Compact human count: 1_284 → "1.3k", 6_700_000 → "6.7M".
fn fmt_big(n: i64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.1}k", n as f64 / 1_000.0) }
    else { n.to_string() }
}

fn net_stat(label: &str, value: usize, cls: &str) -> Markup {
    html! {
        div .net-stat {
            span .net-value class=(cls) { (value) }
            span .net-label { (label) }
        }
    }
}

fn nav_card(href: &str, icon: maud::PreEscaped<&'static str>, title: &str, sub: &str) -> Markup {
    html! {
        a .dash-nav-card href=(href) {
            span .dnc-icon { (icon) }
            h3 { (title) }
            p { (sub) }
        }
    }
}
```

- [ ] **Step 3: Add the inline-SVG icon constants (replacing the old emoji)**

At the bottom of `dashboard.rs`:
```rust
use maud::PreEscaped;

// 20×20 Lucide-style line icons (stroke=currentColor). Decorative → aria-hidden.
const ICON_BOARD: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="14" rx="2"/><path d="M3 9h18M8 18v3M16 18v3"/></svg>"#);
const ICON_CHART: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 3v18h18"/><path d="M7 14l3-4 3 2 4-6"/></svg>"#);
const ICON_TROPHY: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 4h12v3a6 6 0 0 1-12 0V4z"/><path d="M6 6H4v1a3 3 0 0 0 3 3M18 6h2v1a3 3 0 0 1-3 3M9 17h6M10 21h4M12 13v4"/></svg>"#);
const ICON_GEAR: PreEscaped<&'static str> = PreEscaped(r#"<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"#);
```
Delete the old `struct DataCounts`, `struct PredStats`, all the old `fetch_*` functions, and the old `render(...)`. The cockpit reuses `fmt_big` (defined in Step 2's render block) for the coverage strip — keep exactly one `fmt_big`; if the old file already had one, remove the duplicate. (Keep nothing dead — `cargo build` will flag leftovers.)

- [ ] **Step 4: Build + clippy**
```bash
cd RailPredict && cargo build 2>&1 | tail -8
cargo clippy --lib 2>&1 | grep "dashboard.rs" || echo "no dashboard.rs warnings"
```
Expected: clean build, no warnings. Fix any unused-import / dead-code fallout from removing the old code.

- [ ] **Step 5: Commit**
```bash
git add RailPredict/src/frontend/dashboard.rs
git commit -m "feat(ui): rebuild dashboard into live overview cockpit"
```

---

## Task 5: Cockpit CSS + version bump

**Files:**
- Modify: `RailPredict/static/style.css`
- Modify: the five version files

- [ ] **Step 1: Append cockpit CSS to the end of `style.css`**
```css
/* ============================================================
   Signal Terminal — Phase 2 overview cockpit
   ============================================================ */

.dash-header { display:flex; align-items:flex-start; justify-content:space-between; gap:16px; margin-bottom:18px; flex-wrap:wrap; }
.dash-title { font-size:20px; font-weight:600; }
.dash-sub { color:var(--text-muted); font-size:13px; margin-top:2px; }
.kpi-strip { margin-bottom:18px; }

.cockpit-grid { display:grid; grid-template-columns:1fr 1fr; gap:14px; margin-bottom:22px; }
@media (max-width:820px){ .cockpit-grid { grid-template-columns:1fr; } }

.panel { background:var(--surface); border:1px solid var(--border); border-radius:var(--r-md); padding:14px 16px; }
.panel-head { display:flex; align-items:center; justify-content:space-between; margin-bottom:12px; }
.panel-head h2 { font-size:14px; font-weight:600; letter-spacing:.02em; }
.panel-meta { font-size:12px; color:var(--text-muted); }
.panel-link { font-size:12px; color:var(--accent); }
.panel-link:hover { text-decoration:underline; }
.panel-empty { color:var(--text-muted); font-size:13px; padding:10px 0; }

.net-counts { display:grid; grid-template-columns:repeat(4,1fr); gap:8px; margin-bottom:12px; }
.net-stat { display:flex; flex-direction:column; gap:2px; }
.net-value { font-family:var(--font-mono); font-variant-numeric:tabular-nums; font-size:22px; font-weight:600; line-height:1; }
.net-label { font-size:11px; color:var(--text-muted); text-transform:uppercase; letter-spacing:.04em; }
.net-ok { color:var(--ok); } .net-warn { color:var(--warn); } .net-bad { color:var(--bad); } .net-neutral { color:var(--text); }

.mini-table { width:100%; border-collapse:collapse; font-size:13px; }
.mini-table th { text-align:left; font-size:11px; color:var(--text-muted); text-transform:uppercase; letter-spacing:.04em; padding:4px 6px; border-bottom:1px solid var(--border); }
.mini-table td { padding:6px; border-bottom:1px solid var(--border); }
.mini-table tr:last-child td { border-bottom:none; }
.mini-table .num { text-align:right; font-family:var(--font-mono); font-variant-numeric:tabular-nums; }
.mini-table code { color:var(--text); }
.mini-table .arrow { color:var(--text-faint); margin:0 5px; }
.delay-bad { color:var(--bad); font-family:var(--font-mono); }
.op-chip { display:inline-block; width:9px; height:9px; border-radius:2px; margin-right:7px; vertical-align:middle; }

.dash-nav-card .dnc-icon { color:var(--accent); display:inline-flex; }

.coverage-strip { display:flex; flex-wrap:wrap; gap:10px; margin-top:18px; padding-top:14px; border-top:1px solid var(--border); }
.cov-chip { display:flex; flex-direction:column; gap:2px; background:var(--surface); border:1px solid var(--border); border-radius:var(--r-sm); padding:8px 12px; }
.cov-value { font-family:var(--font-mono); font-variant-numeric:tabular-nums; font-size:16px; font-weight:600; }
.cov-label { font-size:11px; color:var(--text-muted); text-transform:uppercase; letter-spacing:.04em; }
```

> This reuses Phase-0 `.kpi-strip`/`.kpi-card` rules. If older dashboard CSS (`.dash-hero`, `.dash-metrics`, `.dm-*`) is now unused after the rewrite, leave it in place (harmless) — do not hunt unrelated CSS.

- [ ] **Step 2: Build (CSS embeds) + run the app smoke if a DB is available**
```bash
cd RailPredict && cargo build 2>&1 | tail -3
```

- [ ] **Step 3: Bump version to 1.12.7 / 03/06/2026 across all five files**

`grep -rn '1.12.6' CLAUDE.md README.md TODO.md CHANGELOG.md RailPredict/Cargo.toml`, then:
- CLAUDE.md / TODO.md / CHANGELOG.md header → `**version = "1.12.7" -- 03/06/2026**`
- README.md → `**v1.12.7 — June 2026**`
- RailPredict/Cargo.toml `[package] version` → `1.12.7`

CHANGELOG entry (top of entries):
```markdown
## [1.12.7] — 2026-06-03
### Changed
- Dashboard overhaul **Phase 2 (Overview cockpit)**: `/` rebuilt into a live Signal-Terminal cockpit — KPI strip (on-time %, avg delay, prediction MAE, trains tracked) with sparklines + trend arrows, re-scopable by the global time-range picker (24h/7d/30d/all via htmx); a live network-state panel (tracked/on-time/delayed/cancelled + worst current delays) from the registry; and a mini operator league (top 8 by on-time %, empty until the GTFS operator ingest runs). New read helpers `db/overview.rs` and `operator_league`, plus `TrainRegistry::network_summary`.
```

- [ ] **Step 4: Full suite + commit**
```bash
cd RailPredict && cargo build 2>&1 | tail -2 && cargo test 2>&1 | grep -E "test result:" | head
cd /Users/miki_przygoda/Projects/GitHub-Projects/RailPredict
git add RailPredict/static/style.css CLAUDE.md README.md TODO.md CHANGELOG.md RailPredict/Cargo.toml RailPredict/Cargo.lock
git commit -m "feat(ui): cockpit CSS; bump v1.12.7 (Phase 2)"
```
Confirm `.gitignore`/`skills-lock.json` are not staged.

---

## Self-Review

- **Spec coverage (§5 `/`):** KPI strip with live data + sparklines (Task 1 + 4), network-state panel (Task 3 + 4), mini operator league (Task 2 + 4), global time-range picker re-scoping via htmx fragment (Task 4 handler), Trends/Model-health folded into the cockpit panels + range (per spec YAGNI). Emoji nav icons replaced with inline SVG (Task 4 Step 3).
- **Placeholder scan:** the only intentional fragment-vs-page branch is the `hx-request` check (Task 4); the Task 3 test has a clearly-marked adaptation point (mirror the existing `TrainStatus` test constructor) — not a placeholder but an instruction to match the real helper. All SQL/Rust/CSS is complete.
- **6.7M-row safety:** all aggregates are read-only `SELECT` with a windowed `WHERE`; the operator league JOINs on `services.uid` (PK). No writes.
- **Type consistency:** `HeadlineMetrics { on_time_pct, avg_delay_mins, mae_mins, sample_count }`, `DailyPoint { on_time_pct, avg_delay_mins, mae_mins }`, `OperatorLeagueRow`, and `NetworkSummary { tracked, on_time, delayed, cancelled, worst }` are referenced identically in the queries, the render, and the tests. `kpi_card(label, value, unit, Option<(f64, Polarity)>, Option<&[f64]>)` matches the Phase-0 signature (post-review-fix). `time_range_picker("/", range)` matches the Phase-0 component.
- **Graceful empties:** `unwrap_or_default()` on each query; league + worst lists have explicit empty states; `fmt_opt` renders `—` for NULL aggregates (no-data windows).

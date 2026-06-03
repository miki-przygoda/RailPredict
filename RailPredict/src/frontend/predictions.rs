//! ML prediction analytics page — `/predictions`
//!
//! Shows a 24-hour summary of model accuracy, a sortable station leaderboard,
//! an hour-of-day accuracy chart, and a table of the biggest recent errors.
//! All data is queried at page load (SSR); JS handles client-side sorting only.

use axum::extract::State;
use maud::{Markup, PreEscaped, html};

use crate::api::AppState;
use crate::db::synthetic::{synthetic_stats, SyntheticStats};

use super::layout::{base, NavPage};

// ---------------------------------------------------------------------------
// Query result types
// ---------------------------------------------------------------------------

struct Summary {
    total:        i64,
    mae:          f64,
    median_ae:    f64,
    pct_5min:     f64,
    bias:         f64,
}

struct StationRow {
    origin_crs: String,
    n:          i64,
    mae:        f64,
    bias:       f64,
    pct_5min:   f64,
}

struct HourRow {
    hour: i64,
    n:    i64,
    mae:  f64,
}

struct ErrorRow {
    uid:        String,
    origin_crs: String,
    actual:     i32,
    predicted:  i32,
    error:      i32,
}

// ---------------------------------------------------------------------------
// Page handler
// ---------------------------------------------------------------------------

pub async fn predictions_page(State(state): State<AppState>) -> Markup {
    let (summary, stations, hours, errors, synth) = tokio::join!(
        query_summary(&state),
        query_stations(&state),
        query_hours(&state),
        query_errors(&state),
        synthetic_stats(&state.db),
    );

    base("Predictions", NavPage::Predictions, render(summary, stations, hours, errors, synth.unwrap_or(None)))
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

async fn query_summary(state: &AppState) -> Option<Summary> {
    sqlx::query_as::<_, (i64, f64, f64, f64, f64)>(
        "SELECT COUNT(*),
                AVG(ABS(predicted_delay_mins - delay_mins))::float8,
                PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY ABS(predicted_delay_mins - delay_mins))::float8,
                (AVG(CASE WHEN ABS(predicted_delay_mins - delay_mins) <= 5 THEN 1.0 ELSE 0.0 END) * 100)::float8,
                AVG((predicted_delay_mins - delay_mins)::float8)
         FROM delay_history
         WHERE recorded_at > NOW() - INTERVAL '24 hours'
           AND predicted_delay_mins IS NOT NULL
           AND delay_mins BETWEEN -120 AND 600
           AND ABS(predicted_delay_mins - delay_mins) < 300",
    )
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .map(|(total, mae, median, pct, bias)| Summary { total, mae, median_ae: median, pct_5min: pct, bias })
}

async fn query_stations(state: &AppState) -> Vec<StationRow> {
    sqlx::query_as::<_, (String, i64, f64, f64, f64)>(
        "SELECT origin_crs,
                COUNT(*),
                AVG(ABS(predicted_delay_mins - delay_mins))::float8,
                AVG((predicted_delay_mins - delay_mins)::float8),
                (AVG(CASE WHEN ABS(predicted_delay_mins - delay_mins) <= 5 THEN 1.0 ELSE 0.0 END) * 100)::float8
         FROM delay_history
         WHERE recorded_at > NOW() - INTERVAL '24 hours'
           AND predicted_delay_mins IS NOT NULL
           AND delay_mins BETWEEN -120 AND 600
           AND ABS(predicted_delay_mins - delay_mins) < 300
         GROUP BY origin_crs
         HAVING COUNT(*) >= 20
         ORDER BY AVG(ABS(predicted_delay_mins - delay_mins)) ASC
         LIMIT 100",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(origin_crs, n, mae, bias, pct_5min)| StationRow { origin_crs, n, mae, bias, pct_5min })
    .collect()
}

async fn query_hours(state: &AppState) -> Vec<HourRow> {
    sqlx::query_as::<_, (f64, i64, f64, f64)>(
        "SELECT EXTRACT(HOUR FROM recorded_at AT TIME ZONE 'Europe/London')::float8,
                COUNT(*),
                AVG(ABS(predicted_delay_mins - delay_mins))::float8,
                AVG((predicted_delay_mins - delay_mins)::float8)
         FROM delay_history
         WHERE recorded_at > NOW() - INTERVAL '7 days'
           AND predicted_delay_mins IS NOT NULL
           AND delay_mins BETWEEN -120 AND 600
           AND ABS(predicted_delay_mins - delay_mins) < 300
         GROUP BY 1
         ORDER BY 1",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(h, n, mae, _bias)| HourRow { hour: h as i64, n, mae })
    .collect()
}

async fn query_errors(state: &AppState) -> Vec<ErrorRow> {
    sqlx::query_as::<_, (String, String, i32, i32, i32)>(
        "SELECT uid, origin_crs, delay_mins, predicted_delay_mins,
                (predicted_delay_mins - delay_mins)
         FROM delay_history
         WHERE recorded_at > NOW() - INTERVAL '6 hours'
           AND predicted_delay_mins IS NOT NULL
           AND delay_mins BETWEEN -120 AND 600
           AND ABS(predicted_delay_mins - delay_mins) BETWEEN 30 AND 299
         ORDER BY ABS(predicted_delay_mins - delay_mins) DESC
         LIMIT 40",
    )
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(uid, origin_crs, actual, predicted, error)| ErrorRow { uid, origin_crs, actual, predicted, error })
    .collect()
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn render(
    summary: Option<Summary>,
    stations: Vec<StationRow>,
    hours: Vec<HourRow>,
    errors: Vec<ErrorRow>,
    synth: Option<SyntheticStats>,
) -> Markup {
    html! {
        div .pred-page {
            div .pred-page-header {
                h1 { "ML Predictions" }
                p .pred-page-subtitle {
                    "Accuracy analytics for the LightGBM delay models — last 24 hours of live data."
                }
            }

            // Summary cards
            @if let Some(s) = &summary {
                div .pred-summary-row {
                    div .pred-stat-card {
                        span .pred-stat-label { "Predictions (24 h)" }
                        span .pred-stat-value { (format_big(s.total)) }
                    }
                    div .pred-stat-card {
                        span .pred-stat-label { "Mean abs. error" }
                        span .pred-stat-value { (format!("{:.1}", s.mae)) " min" }
                    }
                    div .pred-stat-card {
                        span .pred-stat-label { "Median abs. error" }
                        span .pred-stat-value { (format!("{:.0}", s.median_ae)) " min" }
                    }
                    div .pred-stat-card {
                        span .pred-stat-label { "Within ±5 min" }
                        span .pred-stat-value { (format!("{:.0}", s.pct_5min)) "%" }
                    }
                    div .pred-stat-card {
                        span .pred-stat-label { "Model bias" }
                        span .pred-stat-value
                            .pred-bias-over[s.bias > 1.0]
                            .pred-bias-under[s.bias < -1.0]
                        {
                            @if s.bias >= 0.0 {
                                "+" (format!("{:.1}", s.bias)) " min"
                            } @else {
                                (format!("{:.1}", s.bias)) " min"
                            }
                        }
                    }
                }
            } @else {
                p .pred-no-data { "No prediction data for the last 24 hours." }
            }

            // Hour-of-day chart
            @if !hours.is_empty() {
                section .pred-section {
                    h2 .pred-section-title { "Accuracy by hour of day" }
                    p .pred-section-sub { "7-day window — mean absolute error per hour (local time). Taller = worse." }
                    (hour_chart(&hours))
                }
            }

            // Station leaderboard
            @if !stations.is_empty() {
                section .pred-section {
                    h2 .pred-section-title {
                        "Station accuracy"
                        span .pred-section-meta { " — " (stations.len()) " stations, ≥20 predictions in 24 h" }
                    }
                    (station_table(&stations))
                }
            }

            // Biggest errors
            @if !errors.is_empty() {
                section .pred-section {
                    h2 .pred-section-title { "Biggest recent errors" }
                    p .pred-section-sub { "Last 6 hours — predictions off by 30–299 min, sorted by absolute error." }
                    (errors_table(&errors))
                }
            }

            // Synthetic training data
            @if let Some(s) = &synth {
                section .pred-section {
                    h2 .pred-section-title {
                        "Synthetic training data"
                        span .pred-section-meta { " — " (s.generation) }
                    }
                    p .pred-section-sub {
                        "3 synthetic weeks of on-time and average operating days injected into training "
                        "to teach the model normal operations. Service patterns cloned from real Darwin data; "
                        "delay signals and rolling features generated self-consistently per day type."
                    }
                    div .pred-summary-row {
                        div .pred-stat-card {
                            span .pred-stat-label { "Total rows" }
                            span .pred-stat-value { (format_big(s.total_rows)) }
                        }
                        div .pred-stat-card {
                            span .pred-stat-label { "Good-day rows" }
                            span .pred-stat-value { (format_big(s.good_rows)) }
                        }
                        div .pred-stat-card {
                            span .pred-stat-label { "Average-day rows" }
                            span .pred-stat-value { (format_big(s.average_rows)) }
                        }
                        div .pred-stat-card {
                            span .pred-stat-label { "On-time % (good days)" }
                            span .pred-stat-value { (format!("{:.0}", s.ontime_pct_good)) "%" }
                        }
                        div .pred-stat-card {
                            span .pred-stat-label { "On-time % (average days)" }
                            span .pred-stat-value { (format!("{:.0}", s.ontime_pct_average)) "%" }
                        }
                        div .pred-stat-card {
                            span .pred-stat-label { "Avg delay (good / avg)" }
                            span .pred-stat-value {
                                (format!("{:.1}", s.avg_delay_good)) " / "
                                (format!("{:.1}", s.avg_delay_average)) " min"
                            }
                        }
                    }
                }
            }
        }

        script { (PreEscaped(SORT_JS)) }
    }
}

// ---------------------------------------------------------------------------
// Hour-of-day SVG bar chart
// ---------------------------------------------------------------------------

fn hour_chart(hours: &[HourRow]) -> Markup {
    let max_mae = hours.iter().map(|h| h.mae).fold(0.0_f64, f64::max).max(1.0);
    let chart_h  = 100.0_f64;
    let bar_w    = 14.0_f64;
    let gap      = 3.0_f64;
    let total_w  = 24.0 * (bar_w + gap);
    let label_h  = 18.0_f64;
    let svg_h    = chart_h + label_h;

    // Build a lookup by hour
    let mut by_hour: [Option<&HourRow>; 24] = [None; 24];
    for h in hours {
        if h.hour >= 0 && h.hour < 24 {
            by_hour[h.hour as usize] = Some(h);
        }
    }

    html! {
        div .pred-hour-chart {
            svg
                viewBox={ "0 0 " (total_w) " " (svg_h) }
                width="100%"
                style={ "max-width:" (total_w * 2.0) "px" }
                aria-label="Hour-of-day MAE bar chart"
            {
                @for (hr, slot) in by_hour.iter().copied().enumerate() {
                    @let x = hr as f64 * (bar_w + gap);
                    @let (bar_height, colour, title_text) = if let Some(row) = slot {
                        let h = (row.mae / max_mae * chart_h).max(2.0);
                        let c = if row.mae < 5.0 { "#00c896" }
                                else if row.mae < 15.0 { "#f5a624" }
                                else { "#f04f4f" };
                        (h, c, format!("{:02}:00 — MAE {:.1} min ({} trains)", hr, row.mae, row.n))
                    } else {
                        (2.0, "#2a2b38", format!("{:02}:00 — no data", hr))
                    };
                    @let y = chart_h - bar_height;

                    rect
                        x=(format!("{:.1}", x))
                        y=(format!("{:.1}", y))
                        width=(format!("{:.1}", bar_w))
                        height=(format!("{:.1}", bar_height))
                        fill=(colour)
                        rx="2"
                    {
                        title { (title_text) }
                    }

                    // Hour label — every 3 hours
                    @if hr % 3 == 0 {
                        text
                            x=(format!("{:.1}", x + bar_w / 2.0))
                            y=(format!("{:.1}", svg_h - 2.0))
                            text-anchor="middle"
                            font-size="8"
                            fill="#6b6c7e"
                        {
                            (format!("{:02}", hr))
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Station table (sortable)
// ---------------------------------------------------------------------------

fn station_table(stations: &[StationRow]) -> Markup {
    html! {
        div .pred-table-wrap {
            div .pred-sort-bar {
                span .pred-sort-label { "Sort by:" }
                button .pred-sort-btn.active data-col="mae"     onclick="sortTable('mae',this)"     { "MAE ↑" }
                button .pred-sort-btn          data-col="volume" onclick="sortTable('volume',this)"  { "Volume" }
                button .pred-sort-btn          data-col="pct"    onclick="sortTable('pct',this)"     { "Within 5 min %" }
                button .pred-sort-btn          data-col="bias"   onclick="sortTable('bias',this)"    { "Bias" }
            }
            table .pred-table #station-table {
                thead {
                    tr {
                        th .col-crs  { "Station" }
                        th .col-n    { "Trains (24 h)" }
                        th .col-mae  { "MAE" }
                        th .col-bias { "Bias" }
                        th .col-pct  { "Within 5 min" }
                        th .col-bar  { "" }
                    }
                }
                tbody {
                    @for row in stations {
                        @let pct = row.pct_5min;
                        @let bar_width = (pct as u32).min(100);
                        tr
                            data-mae=(format!("{:.2}", row.mae))
                            data-volume=(row.n)
                            data-pct=(format!("{:.1}", row.pct_5min))
                            data-bias=(format!("{:.2}", row.bias.abs()))
                        {
                            td .col-crs { code { (row.origin_crs) } }
                            td .col-n   { (row.n) }
                            td .col-mae {
                                span .pred-mae-value
                                    .pred-mae-good[row.mae < 5.0]
                                    .pred-mae-ok[row.mae >= 5.0 && row.mae < 15.0]
                                    .pred-mae-bad[row.mae >= 15.0]
                                {
                                    (format!("{:.1}", row.mae)) " min"
                                }
                            }
                            td .col-bias {
                                span
                                    .pred-bias-over[row.bias > 1.0]
                                    .pred-bias-under[row.bias < -1.0]
                                {
                                    @if row.bias >= 0.0 { "+" }
                                    (format!("{:.1}", row.bias)) " min"
                                }
                            }
                            td .col-pct { (format!("{:.0}", pct)) "%" }
                            td .col-bar {
                                div .pred-bar-track {
                                    div .pred-bar-fill
                                        style={ "width:" (bar_width) "%" }
                                    {}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Errors table
// ---------------------------------------------------------------------------

fn errors_table(errors: &[ErrorRow]) -> Markup {
    html! {
        table .pred-table .pred-errors-table {
            thead {
                tr {
                    th { "UID" }
                    th { "Station" }
                    th { "Actual" }
                    th { "Predicted" }
                    th { "Error" }
                }
            }
            tbody {
                @for row in errors {
                    tr {
                        td { code { (row.uid) } }
                        td { code { (row.origin_crs) } }
                        td { (row.actual) " min" }
                        td { (row.predicted) " min" }
                        td {
                            span
                                .pred-error-over[row.error > 0]
                                .pred-error-under[row.error < 0]
                            {
                                @if row.error > 0 { "+" }
                                (row.error) " min"
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Client-side sort JS (table rows only — no server round-trip)
// ---------------------------------------------------------------------------

const SORT_JS: &str = r#"
(function () {
    window.sortTable = function (col, btn) {
        document.querySelectorAll('.pred-sort-btn')
            .forEach(function (el) { el.classList.remove('active'); });
        btn.classList.add('active');

        var tbody = document.querySelector('#station-table tbody');
        if (!tbody) return;
        var rows = Array.from(tbody.querySelectorAll('tr'));

        var asc = col === 'mae' || col === 'bias';  // lower is better for these
        rows.sort(function (a, b) {
            var va = parseFloat(a.dataset[col] || '0');
            var vb = parseFloat(b.dataset[col] || '0');
            return asc ? va - vb : vb - va;
        });
        rows.forEach(function (r) { tbody.appendChild(r); });
    };
})();
"#;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_big(n: i64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.0}k", n as f64 / 1_000.0) }
    else { n.to_string() }
}

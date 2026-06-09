//! Predicted-vs-actual explorer — `/predictions`.
//!
//! Aggregate model-accuracy analytics over a rolling window (re-scopable by the
//! global time-range picker): a calibration plot, daily MAE trend, the signed
//! error distribution (bias), MAE by model confidence, and MAE by prediction
//! lead time. All data is read at page load from `db::analytics` + `db::predictions`.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use maud::{Markup, html};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::{analytics, predictions as preds};
use crate::frontend::charts::{self, KpiTone};
use crate::frontend::components::{compact_count, normalize_range, range_label, range_to_hours, time_range_picker};

use super::layout::{base, NavPage};

#[derive(Debug, Deserialize)]
pub struct RangeParams {
    #[serde(default)]
    pub range: Option<String>,
}

pub async fn predictions_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<RangeParams>,
) -> Markup {
    let range = normalize_range(params.range.as_deref());
    let hours = range_to_hours(range);

    let (summary, calib, acc, errs, conf, lead) = tokio::join!(
        preds::accuracy_summary(&state.db, hours),
        analytics::calibration_curve(&state.db, hours),
        analytics::accuracy_over_time(&state.db, hours),
        analytics::error_distribution(&state.db, hours),
        analytics::confidence_error(&state.db, hours),
        preds::leadtime_accuracy(&state.db, hours),
    );

    let body = render(
        range,
        summary.ok(),
        calib.unwrap_or_default(),
        acc.unwrap_or_default(),
        errs.unwrap_or_default(),
        conf.unwrap_or_default(),
        lead.unwrap_or_default(),
    );
    if headers.contains_key("hx-request") {
        body
    } else {
        base("Predictions", NavPage::Predictions, body)
    }
}

// ---------------------------------------------------------------------------
// Formatting + tone helpers
// ---------------------------------------------------------------------------

fn fmt1(v: Option<f64>) -> String {
    match v {
        Some(x) => format!("{x:.1}"),
        None => "—".to_string(),
    }
}

/// Bar tone by mean-absolute-error magnitude.
fn mae_tone(mae: f64) -> &'static str {
    if mae < 5.0 { "bar-ok" } else if mae < 10.0 { "bar-info" } else if mae < 15.0 { "bar-warn" } else { "bar-bad" }
}

/// One vertical bar: a label, a value caption, a 0–1 height fraction, and a tone class.
struct VBar {
    label: &'static str,
    value: String,
    frac: f64,
    cls: &'static str,
}

fn vbar_chart(bars: &[VBar]) -> Markup {
    html! {
        @if bars.is_empty() {
            p .panel-empty { "No data in this window yet." }
        } @else {
            div .vbars {
                @for b in bars {
                    div .col {
                        span .v { (b.value) }
                        div class=(format!("bar {}", b.cls))
                            style=(format!("height:{}%", (b.frac.clamp(0.0, 1.0) * 100.0).round() as i64)) {}
                        div .lbl { (b.label) }
                    }
                }
            }
        }
    }
}

/// Label + tone for a signed-error band (negative = over-predicted, positive = under-predicted).
fn error_band(lower: i32) -> (&'static str, &'static str) {
    match lower {
        -999 => ("≤−15", "bar-bad"),
        -15 => ("−15…−5", "bar-warn"),
        -5 => ("−5…−2", "bar-info"),
        -2 => ("−2…2", "bar-ok"),
        2 => ("2…5", "bar-info"),
        5 => ("5…15", "bar-warn"),
        _ => ("15+", "bar-bad"),
    }
}

/// Label for a confidence band keyed on its lower edge (0.0/0.2/0.4/0.6/0.8).
fn conf_band(lower: f64) -> &'static str {
    match (lower * 10.0).round() as i64 {
        0 => "0–20%",
        2 => "20–40%",
        4 => "40–60%",
        6 => "60–80%",
        _ => "80–100%",
    }
}

/// Label for a lead-time band keyed on its lower edge in minutes.
fn lead_band(lower: i32) -> &'static str {
    match lower {
        0 => "<15m",
        15 => "15–30",
        30 => "30–60",
        60 => "60–120",
        _ => "120+",
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn render(
    range: &str,
    summary: Option<preds::AccuracySummary>,
    calib: Vec<analytics::CalibrationPoint>,
    acc: Vec<analytics::AccuracyPoint>,
    errs: Vec<analytics::ErrorBucket>,
    conf: Vec<analytics::ConfidenceBucket>,
    lead: Vec<preds::LeadTimeBucket>,
) -> Markup {
    let scored = summary.as_ref().map(|s| s.finalised_count).unwrap_or(0);
    let within5 = summary
        .as_ref()
        .filter(|s| s.finalised_count > 0)
        .map(|s| s.within_5_count as f64 / s.finalised_count as f64 * 100.0);
    let mae = summary.as_ref().and_then(|s| s.mean_abs_error_mins);
    // Mean signed error in the same direction as the error-distribution chart
    // (actual − predicted): positive = trains ran later than forecast on average.
    let bias = summary.as_ref().and_then(|s| match (s.mean_predicted_mins, s.mean_actual_mins) {
        (Some(p), Some(a)) => Some(a - p),
        _ => None,
    });

    // Calibration points (predicted, actual), dropping bands with NULL means.
    let calib_pts: Vec<(f64, f64)> = calib
        .iter()
        .filter_map(|c| Some((c.mean_predicted?, c.mean_actual?)))
        .collect();

    // Daily MAE for the trend.
    let mae_series: Vec<f64> = acc.iter().filter_map(|p| p.mae_mins).collect();

    // Signed-error bars (share of finalised rows per band).
    let err_total: i64 = errs.iter().map(|b| b.sample_count).sum::<i64>().max(1);
    let err_bars: Vec<VBar> = errs
        .iter()
        .map(|b| {
            let (label, cls) = error_band(b.lower_bound_mins);
            let pct = b.sample_count as f64 / err_total as f64 * 100.0;
            VBar { label, value: format!("{}%", pct.round() as i64), frac: pct / 100.0, cls }
        })
        .collect();

    // Confidence-vs-error bars (height ∝ MAE; ideally descending).
    let conf_max = conf.iter().filter_map(|b| b.mae_mins).fold(1.0_f64, f64::max);
    let conf_bars: Vec<VBar> = conf
        .iter()
        .map(|b| {
            let m = b.mae_mins.unwrap_or(0.0);
            VBar { label: conf_band(b.confidence_lower), value: fmt1(b.mae_mins), frac: m / conf_max, cls: mae_tone(m) }
        })
        .collect();

    // Lead-time bars (height ∝ MAE; ideally ascending toward longer lead).
    let lead_max = lead.iter().filter_map(|b| b.mae_mins).fold(1.0_f64, f64::max);
    let lead_bars: Vec<VBar> = lead
        .iter()
        .map(|b| {
            let m = b.mae_mins.unwrap_or(0.0);
            VBar { label: lead_band(b.lower_bound_mins), value: fmt1(b.mae_mins), frac: m / lead_max, cls: mae_tone(m) }
        })
        .collect();

    html! {
        div .predictions {
            div .dash-header {
                div {
                    h1 .dash-title { "Predictions" }
                    p .dash-sub { "Predicted vs actual · model accuracy · " (range_label(range)) }
                }
                div .dash-header-right { (time_range_picker("/predictions", range)) }
            }

            div .kpi-strip {
                (charts::kpi_card("Forecasts scored", &compact_count(scored, 0), None,
                    Some("completed & checked vs reality"), None, None, KpiTone::Neutral))
                (charts::kpi_card("Within 5 min", &fmt1(within5), Some("%"),
                    Some("forecasts within 5 min of actual"), None, None, KpiTone::Ok))
                (charts::kpi_card("Average miss", &fmt1(mae), Some("min"),
                    Some("typical gap, forecast vs actual"), None, None, KpiTone::Info))
                (charts::kpi_card("Lean", &bias.map(|b| format!("{b:+.1}")).unwrap_or_else(|| "—".into()), Some("min"),
                    Some("+ ran later, − earlier than forecast"), None, None, KpiTone::Warn))
            }

            div .cockpit-grid {
                section .panel {
                    div .panel-head { h2 { "Calibration" } span .panel-meta { "forecast vs reality" } }
                    div .panel-body {
                        @if calib_pts.len() >= 2 {
                            div .calib-wrap { (charts::calibration_plot(&calib_pts)) }
                            div .legend {
                                span .ideal { "forecast = reality" }
                                span .actual { "our forecasts" }
                            }
                            p .chart-note { "Each dot groups similar-sized forecasts: the average forecast (across) vs what actually happened (up). On the line = matched reality; above = trains ran later than forecast; below = earlier." }
                        } @else {
                            p .panel-empty { "Not enough scored predictions for a calibration curve yet." }
                        }
                    }
                }

                section .panel {
                    div .panel-head { h2 { "Accuracy over time" } span .panel-meta { "average miss per day" } }
                    div .panel-body {
                        @if mae_series.len() >= 2 {
                            div .acc-chart.chart-info { (charts::area_spark(&mae_series, "pred-acc")) }
                            p .chart-note { "How far off the forecasts were each day — lower is better · " (range_label(range)) "." }
                        } @else {
                            p .panel-empty { "Not enough days in range for a trend." }
                        }
                    }
                }
            }

            div .pred-grid-3 {
                section .panel {
                    div .panel-head { h2 { "How far off" } }
                    div .panel-body {
                        (vbar_chart(&err_bars))
                        p .chart-note { "Each forecast's gap from reality (actual − forecast). Centre = spot on; bars to the right = trains ran later than forecast; left = earlier." }
                    }
                }
                section .panel {
                    div .panel-head { h2 { "Confidence vs accuracy" } }
                    div .panel-body {
                        (vbar_chart(&conf_bars))
                        p .chart-note { "Average miss grouped by how sure the model was. If confidence is meaningful, the bars should fall left → right — more confident, more accurate." }
                    }
                }
                section .panel {
                    div .panel-head { h2 { "Lead-time accuracy" } }
                    div .panel-body {
                        (vbar_chart(&lead_bars))
                        p .chart-note { "Average miss by how far ahead the forecast was made — forecasts made further out naturally drift more." }
                    }
                }
            }
        }
    }
}

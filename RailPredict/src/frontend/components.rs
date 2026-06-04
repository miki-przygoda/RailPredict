//! Reusable maud fragments and formatting helpers shared across pages.

use maud::{Markup, html};

/// Format pence as a pounds string, e.g. `1299 → "£12.99"`.
pub fn pence_to_pounds(pence: i32) -> String {
    format!("£{:.2}", pence as f64 / 100.0)
}

/// Compact human count: `1_284 → "1.3k"`, `6_700_000 → "6.7M"`.
/// `k_decimals` controls the thousands precision (millions always use 1 dp).
pub fn compact_count(n: i64, k_decimals: usize) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.*}k", k_decimals, n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Coloured status badge: green / amber / red driven by delay value.
pub fn delay_badge(delay_mins: Option<i32>, is_cancelled: bool) -> Markup {
    html! {
        @if is_cancelled {
            span .badge.cancelled { "Cancelled" }
        } @else if let Some(mins) = delay_mins {
            @if mins <= 0 {
                span .badge.on-time { "On time" }
            } @else if mins < 10 {
                span .badge.delayed { (mins) " min" }
            } @else {
                span .badge.late { (mins) " min" }
            }
        } @else {
            span .badge.on-time { "On time" }
        }
    }
}

/// ML prediction chip: shows predicted delay and, when `actual` is also known,
/// the accuracy delta (predicted − actual).
pub fn prediction_chip(predicted: Option<i32>, actual: Option<i32>) -> Markup {
    let Some(pred) = predicted else {
        return html! {};
    };
    html! {
        span .pred-chip {
            @if pred <= 0 {
                span .pred-value.pred-ontime { "Pred: on time" }
            } @else {
                span .pred-value { "Pred: " (pred) " min" }
            }
            @if let Some(act) = actual {
                @let delta = pred - act;
                @if delta == 0 {
                    span .pred-delta.pred-delta-exact { "Δ 0" }
                } @else if delta > 0 {
                    span .pred-delta.pred-delta-over { "Δ +" (delta) }
                } @else {
                    span .pred-delta.pred-delta-under { "Δ " (delta) }
                }
            }
        }
    }
}

/// Large-type platform indicator — Uber-style "your platform is X" moment.
/// `is_planned = true` renders a muted chip with a "Planned" label when the platform
/// comes from the static timetable and has not yet been confirmed by a live source.
/// Renders nothing when no platform is known.
pub fn platform_chip(platform: Option<&str>, is_planned: bool) -> Markup {
    html! {
        @if let Some(p) = platform {
            @if is_planned {
                div .platform-chip.planned {
                    span .platform-label { "Platform (planned)" }
                    span .platform-number { (p) }
                }
            } @else {
                div .platform-chip {
                    span .platform-label { "Platform" }
                    span .platform-number { (p) }
                }
            }
        }
    }
}

/// Global time-range picker. `base_path` is the page it re-scopes (htmx swaps
/// the page `<main>`), `active` is one of "24h" | "7d" | "30d" | "all".
pub fn time_range_picker(base_path: &str, active: &str) -> Markup {
    let ranges = [("24h", "24h"), ("7d", "7d"), ("30d", "30d"), ("all", "All")];
    html! {
        div .range-picker role="group" aria-label="Time range" {
            @for (val, label) in ranges {
                button
                    class=(if val == active { "active" } else { "" })
                    hx-get=(format!("{base_path}?range={val}"))
                    hx-target="main"
                    hx-push-url="true"
                    aria-pressed=(if val == active { "true" } else { "false" })
                    { (label) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_picker_marks_active_and_targets_path() {
        let m = time_range_picker("/", "7d").into_string();
        assert!(m.contains("range-picker"));
        assert!(m.contains("hx-get=\"/?range=24h\""));
        assert!(m.contains("hx-get=\"/?range=7d\""));
        assert!(m.contains("active"));
    }
}

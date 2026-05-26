//! Reusable maud fragments used across search and detail pages.

use maud::{Markup, html};

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

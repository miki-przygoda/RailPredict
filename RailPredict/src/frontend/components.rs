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

/// Large-type platform indicator — Uber-style "your platform is X" moment.
/// Renders nothing when no platform is known.
pub fn platform_chip(platform: Option<&str>) -> Markup {
    html! {
        @if let Some(p) = platform {
            div .platform-chip {
                span .platform-label { "Platform" }
                span .platform-number { (p) }
            }
        }
    }
}

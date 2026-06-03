//! Reusable server-rendered inline-SVG chart helpers ("Signal Terminal").
//!
//! Every helper returns `maud::Markup` containing pure inline SVG/HTML — no JS,
//! no external requests, htmx-swappable. Colour comes from `currentColor` so the
//! caller controls semantics via CSS classes (`--ok` / `--warn` / `--bad`).

use maud::{Markup, html};

/// Whether a rising value is good or bad — controls the red/green of a trend arrow.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    /// Higher is better (e.g. on-time %): up = green.
    HigherIsBetter,
    /// Lower is better (e.g. delay minutes): down = green. This is the default convention.
    LowerIsBetter,
}

/// Small triangular trend indicator. `polarity` decides whether a rising value
/// is coloured good (green) or bad (red). Shape (up/down/flat) always reflects
/// the sign so colour is never the sole signal.
pub fn trend_arrow(delta: f64, polarity: Polarity) -> Markup {
    let (dir, path) = if delta > 0.0 {
        ("trend-up", "M5 2 L9 8 L1 8 Z")
    } else if delta < 0.0 {
        ("trend-down", "M1 2 L9 2 L5 8 Z")
    } else {
        ("trend-flat", "M1 5 H9")
    };
    let good = if polarity == Polarity::HigherIsBetter { "good-up " } else { "" };
    let label = if delta == 0.0 { "0".to_string() } else { format!("{delta:+.1}") };
    html! {
        span class=(format!("trend {good}{dir}")) {
            svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true" {
                path d=(path) fill="currentColor" stroke="currentColor" {}
            }
            span .trend-val { (label) }
        }
    }
}

/// Minimal sparkline (120×28 viewBox, non-scaling stroke). Renders an empty SVG
/// frame for empty input so layout never shifts.
pub fn sparkline(values: &[f64]) -> Markup {
    const W: f64 = 120.0;
    const H: f64 = 28.0;
    const PAD: f64 = 2.0;
    if values.is_empty() {
        return html! { svg .spark width=(W) height=(H) viewBox=(format!("0 0 {W} {H}")) role="img" aria-label="no data" {} };
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };
    let n = values.len();
    let dx = if n > 1 { (W - 2.0 * PAD) / (n as f64 - 1.0) } else { 0.0 };
    let points = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = PAD + dx * i as f64;
            let y = PAD + (H - 2.0 * PAD) * (1.0 - (v - min) / range);
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    html! {
        svg .spark width=(W) height=(H) viewBox=(format!("0 0 {W} {H}")) preserveAspectRatio="none" role="img" aria-label="trend sparkline" {
            polyline points=(points) fill="none" stroke="currentColor" stroke-width="1.5" vector-effect="non-scaling-stroke" {}
        }
    }
}

/// KPI stat card: big mono figure + optional unit, trend (with polarity), and sparkline.
pub fn kpi_card(label: &str, value: &str, unit: Option<&str>, delta: Option<(f64, Polarity)>, spark: Option<&[f64]>) -> Markup {
    html! {
        div .kpi-card {
            div .kpi-label { (label) }
            div .kpi-value {
                span .kpi-number { (value) }
                @if let Some(u) = unit { span .kpi-unit { (u) } }
            }
            div .kpi-foot {
                @if let Some((d, pol)) = delta { (trend_arrow(d, pol)) }
                @if let Some(s) = spark { span .kpi-spark { (sparkline(s)) } }
            }
        }
    }
}

/// Inline horizontal bar for table cells. Fraction clamped to [0,1].
pub fn bar_cell(fraction: f64) -> Markup {
    let pct = (fraction.clamp(0.0, 1.0) * 100.0).round() as i64;
    html! {
        span .bar-cell { span .bar-fill style=(format!("width:{pct}%")) {} }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_empty_has_no_polyline() {
        let s = sparkline(&[]).into_string();
        assert!(s.contains("<svg"), "still renders an svg frame: {s}");
        assert!(!s.contains("<polyline"), "no polyline for empty data: {s}");
    }

    #[test]
    fn sparkline_plots_all_points() {
        let s = sparkline(&[1.0, 5.0, 2.0, 8.0]).into_string();
        assert!(s.contains("<polyline"), "has a polyline: {s}");
        let pts = s.split("points=\"").nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(pts.split(' ').count(), 4, "four coordinate pairs: {pts}");
    }

    #[test]
    fn trend_arrow_direction_classes() {
        assert!(trend_arrow(2.5, Polarity::LowerIsBetter).into_string().contains("trend-up"));
        assert!(trend_arrow(-2.5, Polarity::LowerIsBetter).into_string().contains("trend-down"));
        assert!(trend_arrow(0.0, Polarity::LowerIsBetter).into_string().contains("trend-flat"));
        assert!(trend_arrow(2.5, Polarity::LowerIsBetter).into_string().contains("+2.5"));
    }

    #[test]
    fn trend_arrow_flat_shows_zero_without_sign() {
        let s = trend_arrow(0.0, Polarity::LowerIsBetter).into_string();
        assert!(!s.contains("+0.0"), "flat should not show +0.0: {s}");
        assert!(s.contains(">0<") || s.contains("\"trend-val\">0"), "flat shows plain 0: {s}");
    }

    #[test]
    fn trend_arrow_polarity_emits_good_up() {
        assert!(trend_arrow(1.0, Polarity::HigherIsBetter).into_string().contains("good-up"));
        assert!(!trend_arrow(1.0, Polarity::LowerIsBetter).into_string().contains("good-up"));
    }

    #[test]
    fn bar_cell_clamps_and_sets_width() {
        assert!(bar_cell(0.5).into_string().contains("width:50%"));
        assert!(bar_cell(1.7).into_string().contains("width:100%"));
        assert!(bar_cell(-0.3).into_string().contains("width:0%"));
    }

    #[test]
    fn kpi_card_renders_label_value_unit() {
        let m = kpi_card("On-time", "92.4", Some("%"), Some((-1.2, Polarity::LowerIsBetter)), Some(&[1.0, 2.0, 3.0])).into_string();
        assert!(m.contains("On-time"));
        assert!(m.contains("92.4"));
        assert!(m.contains("kpi-unit"));
        assert!(m.contains("trend-down"));
        assert!(m.contains("<polyline"));
    }
}

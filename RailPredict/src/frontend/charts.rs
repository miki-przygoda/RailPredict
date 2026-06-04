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

/// Gradient-filled sparkline: the line plus a soft area fill fading to transparent.
/// Colour comes from `currentColor` (caller sets it via a CSS class), so one helper
/// serves every tone. `grad_id` must be unique per page — duplicate SVG gradient ids
/// would all resolve to the first, so callers derive it from the card label.
pub fn area_spark(values: &[f64], grad_id: &str) -> Markup {
    const W: f64 = 120.0;
    const H: f64 = 30.0;
    const PAD: f64 = 2.0;
    if values.len() < 2 {
        return html! { svg .area-spark width=(W) height=(H) viewBox=(format!("0 0 {W} {H}")) role="img" aria-label="no data" {} };
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };
    let n = values.len();
    let dx = (W - 2.0 * PAD) / (n as f64 - 1.0);
    let pts: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = PAD + dx * i as f64;
            let y = PAD + (H - 2.0 * PAD) * (1.0 - (v - min) / range);
            (x, y)
        })
        .collect();
    let line = pts.iter().map(|(x, y)| format!("{x:.1},{y:.1}")).collect::<Vec<_>>().join(" ");
    let (first_x, _) = pts[0];
    let (last_x, _) = pts[pts.len() - 1];
    let mut area = format!("M{:.1},{:.1} ", pts[0].0, pts[0].1);
    for (x, y) in &pts[1..] {
        area.push_str(&format!("L{x:.1},{y:.1} "));
    }
    area.push_str(&format!("L{last_x:.1},{H:.1} L{first_x:.1},{H:.1} Z"));
    html! {
        svg .area-spark width=(W) height=(H) viewBox=(format!("0 0 {W} {H}")) preserveAspectRatio="none" role="img" aria-label="trend sparkline" {
            defs {
                linearGradient id=(grad_id) x1="0" x2="0" y1="0" y2="1" {
                    stop offset="0" stop-color="currentColor" stop-opacity="0.32" {}
                    stop offset="1" stop-color="currentColor" stop-opacity="0" {}
                }
            }
            path d=(area) fill=(format!("url(#{grad_id})")) {}
            polyline points=(line) fill="none" stroke="currentColor" stroke-width="1.6" vector-effect="non-scaling-stroke" {}
        }
    }
}

/// Visual tone for a KPI card — drives the accent edge and sparkline colour.
/// Tones map to the data semantic scale (`--ok`/`--warn`/`--bad`/`--info`); `Neutral`
/// uses the muted text colour for figures that don't carry a good/bad judgement.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KpiTone {
    Neutral,
    Ok,
    Warn,
    Bad,
    Info,
}

impl KpiTone {
    fn class(self) -> &'static str {
        match self {
            KpiTone::Neutral => "kpi-neutral",
            KpiTone::Ok => "kpi-ok",
            KpiTone::Warn => "kpi-warn",
            KpiTone::Bad => "kpi-bad",
            KpiTone::Info => "kpi-info",
        }
    }
}

/// KPI stat card: big mono figure + optional unit, optional caption, trend (with
/// polarity), and a tone-coloured area sparkline. `tone` colours the accent edge and
/// the sparkline; the gradient id is derived from `label` so multiple cards don't clash.
#[allow(clippy::too_many_arguments)]
pub fn kpi_card(
    label: &str,
    value: &str,
    unit: Option<&str>,
    caption: Option<&str>,
    delta: Option<(f64, Polarity)>,
    spark: Option<&[f64]>,
    tone: KpiTone,
) -> Markup {
    let grad_id = format!(
        "spk-{}",
        label.to_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), "-")
    );
    html! {
        div class=(format!("kpi-card {}", tone.class())) {
            div .kpi-label { (label) }
            div .kpi-value {
                span .kpi-number { (value) }
                @if let Some(u) = unit { span .kpi-unit { (u) } }
            }
            @if let Some(c) = caption { div .kpi-cap { (c) } }
            div .kpi-foot {
                @if let Some((d, pol)) = delta { (trend_arrow(d, pol)) }
                @if let Some(s) = spark { span .kpi-spark { (area_spark(s, &grad_id)) } }
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

/// Calibration plot: each `(predicted, actual)` mean is placed against the
/// perfect-calibration diagonal (`actual == predicted`). A model line below the
/// diagonal under-predicts; above it over-predicts. Square SVG scaled to the max
/// of the two axes; colours come from the `.calib-*` CSS classes. Needs ≥2 points.
pub fn calibration_plot(points: &[(f64, f64)]) -> Markup {
    const SZ: f64 = 220.0;
    const PAD: f64 = 8.0;
    if points.len() < 2 {
        return html! { svg .calib-svg width=(SZ) height=(SZ) viewBox=(format!("0 0 {SZ} {SZ}")) role="img" aria-label="no data" {} };
    }
    // Sort by predicted value so the polyline never crosses itself, regardless of
    // caller ordering.
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let max = pts.iter().flat_map(|&(p, a)| [p, a]).fold(1.0_f64, f64::max);
    let sx = |v: f64| PAD + (SZ - 2.0 * PAD) * (v / max);
    let sy = |v: f64| SZ - PAD - (SZ - 2.0 * PAD) * (v / max);
    let line: String = pts
        .iter()
        .map(|&(p, a)| format!("{:.1},{:.1}", sx(p), sy(a)))
        .collect::<Vec<_>>()
        .join(" ");
    html! {
        svg .calib-svg width=(SZ) height=(SZ) viewBox=(format!("0 0 {SZ} {SZ}")) role="img" aria-label="calibration: predicted vs actual delay" {
            line .calib-ideal x1=(format!("{:.1}", sx(0.0))) y1=(format!("{:.1}", sy(0.0)))
                x2=(format!("{:.1}", sx(max))) y2=(format!("{:.1}", sy(max))) stroke-dasharray="5 5" {}
            polyline .calib-line points=(line) {}
            @for &(p, a) in points {
                circle .calib-dot cx=(format!("{:.1}", sx(p))) cy=(format!("{:.1}", sy(a))) r="3.5" {}
            }
        }
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
        let m = kpi_card(
            "On-time",
            "92.4",
            Some("%"),
            Some("live movement sample"),
            Some((-1.2, Polarity::LowerIsBetter)),
            Some(&[1.0, 2.0, 3.0]),
            KpiTone::Ok,
        )
        .into_string();
        assert!(m.contains("On-time"));
        assert!(m.contains("92.4"));
        assert!(m.contains("kpi-unit"));
        assert!(m.contains("kpi-cap"), "renders the caption");
        assert!(m.contains("kpi-ok"), "applies the tone class");
        assert!(m.contains("trend-down"));
        assert!(m.contains("<polyline"));
    }

    #[test]
    fn area_spark_has_unique_gradient_and_closed_area() {
        let s = area_spark(&[1.0, 5.0, 2.0, 8.0], "spk-test").into_string();
        assert!(s.contains("linearGradient id=\"spk-test\""), "unique gradient id: {s}");
        assert!(s.contains("url(#spk-test)"), "area fills via the gradient: {s}");
        assert!(s.contains("<polyline"), "line drawn on top: {s}");
        assert!(s.trim_end().contains("Z\""), "area path closes: {s}");
    }

    #[test]
    fn area_spark_empty_is_blank_frame() {
        let s = area_spark(&[], "spk-x").into_string();
        assert!(s.contains("<svg"), "still renders a frame: {s}");
        assert!(!s.contains("<polyline"), "no line for empty data: {s}");
    }

    #[test]
    fn calibration_plot_draws_diagonal_line_and_dots() {
        let m = calibration_plot(&[(0.0, 0.5), (5.0, 4.0), (20.0, 16.0)]).into_string();
        assert!(m.contains("calib-ideal"), "has the perfect-calibration diagonal: {m}");
        assert!(m.contains("calib-line"), "has the model line");
        assert_eq!(m.matches("calib-dot").count(), 3, "one dot per point");
    }

    #[test]
    fn calibration_plot_too_few_points_is_blank() {
        let m = calibration_plot(&[(1.0, 1.0)]).into_string();
        assert!(m.contains("<svg"), "still a frame");
        assert!(!m.contains("calib-line"), "no model line for <2 points");
    }
}

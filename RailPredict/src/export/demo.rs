//! Self-contained exec demo site (`export-demo` CLI subcommand).
//!
//! Bakes real KPIs and a DB-reconstructed predicted→actual replay into one
//! offline HTML file for a CEO sales demo. The replay frames match the
//! `static/board.js` renderer contract exactly so the same renderer (inlined,
//! light-themed) drives them. Real data is shown as real; the "with your
//! ticketing data" figures are clearly labelled projected.

use std::path::Path;

use chrono::Utc;
use serde::Serialize;

use crate::db::Db;

/// A train still being tracked — shows the prediction only (no actual yet).
/// Field names mirror `static/board.js` `trackCard()`.
#[derive(Serialize, Clone, Debug)]
pub struct TrackCard {
    pub rid: String,
    pub operator: String,
    pub brand: String,
    pub label: String,
    pub origin: String,
    pub dest: String,
    pub scheduled: String,
    pub predicted: i32,
}

/// A settled train — shows predicted → actual and the absolute error.
/// Field names mirror `static/board.js` `settledCard()`.
#[derive(Serialize, Clone, Debug)]
pub struct SettledCard {
    pub rid: String,
    pub operator: String,
    pub brand: String,
    pub label: String,
    pub origin: String,
    pub dest: String,
    pub predicted: i32,
    pub actual: i32,
    pub delta: i32,
}

/// One board snapshot — the unit the renderer consumes.
#[derive(Serialize, Clone, Debug)]
pub struct Frame {
    pub tracking: Vec<TrackCard>,
    pub settled: Vec<SettledCard>,
}

/// One real settled outcome, the raw material for the replay timeline.
#[derive(Clone, Debug)]
pub struct ReplayTrain {
    pub rid: String,
    pub operator: String,
    pub brand: String,
    pub label: String,
    pub origin: String,
    pub dest: String,
    pub scheduled: String,
    pub predicted: i32,
    pub actual: i32,
}

fn to_track(t: &ReplayTrain) -> TrackCard {
    TrackCard {
        rid: t.rid.clone(), operator: t.operator.clone(), brand: t.brand.clone(),
        label: t.label.clone(), origin: t.origin.clone(), dest: t.dest.clone(),
        scheduled: t.scheduled.clone(), predicted: t.predicted,
    }
}

fn to_settled(t: &ReplayTrain) -> SettledCard {
    SettledCard {
        rid: t.rid.clone(), operator: t.operator.clone(), brand: t.brand.clone(),
        label: t.label.clone(), origin: t.origin.clone(), dest: t.dest.clone(),
        predicted: t.predicted, actual: t.actual, delta: (t.actual - t.predicted).abs(),
    }
}

/// Slide a window across the trains: at step `i`, trains `[i .. i+tracking_window)`
/// are still tracking (prediction only) and the previous `settled_window` trains
/// are shown settled, most-recent first. Produces `trains.len()+1` candidate frames;
/// fully-empty frames are dropped.
pub fn build_frames(
    trains: &[ReplayTrain],
    tracking_window: usize,
    settled_window: usize,
) -> Vec<Frame> {
    let mut frames = Vec::new();
    for i in 0..=trains.len() {
        let settled_start = i.saturating_sub(settled_window);
        let settled: Vec<SettledCard> = trains[settled_start..i].iter().rev().map(to_settled).collect();
        let tracking_end = (i + tracking_window).min(trains.len());
        let tracking: Vec<TrackCard> = trains[i..tracking_end].iter().map(to_track).collect();
        if tracking.is_empty() && settled.is_empty() {
            continue;
        }
        frames.push(Frame { tracking, settled });
    }
    frames
}

const DEMO_TEMPLATE: &str = include_str!("demo_template.html");
const DEMO_JS: &str = include_str!("demo.js");

#[derive(Serialize, Clone, Debug)]
pub struct OperatorHighlight {
    pub name: String,
    pub brand: String,
    pub on_time_pct: f64,
    pub journeys: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct DemoData {
    pub generated_at: String,
    pub hero_number: String,
    pub hero_observations: i64,
    pub services_count: i64,
    /// `services_count` formatted compactly for display (e.g. "51K+").
    pub services_label: String,
    pub on_time_pct: Option<f64>,
    pub mae_mins: Option<f64>,
    pub within_5_pct: Option<f64>,
    /// Destination arrival reliability (journeys arriving within 5 min), from `journeys`.
    pub arrival_on_time_pct: Option<f64>,
    /// Share of journeys that shed ≥2 min of delay en route, from `journeys`.
    pub recovered_pct: Option<f64>,
    pub operators: Vec<OperatorHighlight>,
    pub frames: Vec<Frame>,
}

/// Format a count as a compact headline string: 2_546_226 -> "2.5M+".
pub fn human_count(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M+", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.0}K+", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Inject the data as a JSON literal into the template. Pure — no DB.
/// Escapes `</` so a stray `</script>` in a string field can't break out of the
/// host <script> (mirrors `export::render_html`).
pub fn render_demo_html(data: &DemoData) -> anyhow::Result<String> {
    let json = serde_json::to_string(data)?.replace("</", "<\\/");
    let html = DEMO_TEMPLATE
        .replace("__DEMO_DATA__", &json)
        .replace("/* __DEMO_JS__ */", DEMO_JS);
    Ok(html)
}

use sqlx::FromRow;

#[derive(FromRow)]
struct KpiRow {
    total_observations: i64,
    total_services: i64,
    on_time_pct: Option<f64>,
    mae_mins: Option<f64>,
    within_5_pct: Option<f64>,
}

#[derive(FromRow)]
struct OperatorRow {
    name: Option<String>,
    on_time_pct: Option<f64>,
    journeys: i64,
}

#[derive(FromRow)]
struct ReplayRow {
    rid: String,
    uid: String,
    operator: Option<String>,
    origin_crs: String,
    destination_crs: Option<String>,
    scheduled_departure: chrono::DateTime<Utc>,
    predicted_delay_mins: i32,
    final_delay_mins: i32,
}

async fn query_kpis(db: &Db, days: i32) -> anyhow::Result<KpiRow> {
    let row = sqlx::query_as::<_, KpiRow>(
        r#"
        SELECT
            COUNT(*)                                                        AS total_observations,
            COUNT(DISTINCT uid || '|' || origin_crs)                        AS total_services,
            (AVG(CASE WHEN delay_mins <= 5 THEN 1.0 ELSE 0.0 END) * 100)::float8
                                                                            AS on_time_pct,
            AVG(ABS(delay_mins - predicted_delay_mins)::float8)
                FILTER (WHERE predicted_delay_mins IS NOT NULL)             AS mae_mins,
            (AVG(CASE WHEN predicted_delay_mins IS NOT NULL
                      AND ABS(delay_mins - predicted_delay_mins) <= 5
                 THEN 1.0 ELSE 0.0 END)
             FILTER (WHERE predicted_delay_mins IS NOT NULL) * 100)::float8 AS within_5_pct
        FROM delay_history
        WHERE recorded_at > NOW() - $1::INT * INTERVAL '1 day'
          AND delay_mins BETWEEN -120 AND 600
        "#,
    )
    .bind(days)
    .fetch_one(db)
    .await?;
    Ok(row)
}

async fn query_operator_highlights(db: &Db, days: i32) -> anyhow::Result<Vec<OperatorHighlight>> {
    let rows = sqlx::query_as::<_, OperatorRow>(
        r#"
        SELECT
            COALESCE(op.name, j.toc)                                          AS name,
            (AVG(CASE WHEN COALESCE(j.arrival_delay_mins, j.origin_delay_mins) <= 5
                 THEN 1.0 ELSE 0.0 END) * 100)::float8                        AS on_time_pct,
            COUNT(*)                                                          AS journeys
        FROM journeys j
        LEFT JOIN operators op ON op.toc = j.toc
        WHERE j.scheduled_departure > NOW() - $1::INT * INTERVAL '1 day'
          AND j.toc IS NOT NULL AND j.toc <> ''
        GROUP BY j.toc, op.name
        HAVING COUNT(*) >= 20
        ORDER BY on_time_pct DESC NULLS LAST
        LIMIT 6
        "#,
    )
    .bind(days)
    .fetch_all(db)
    .await?;
    // Brand colour is derived from the operator name via the curated mapping
    // (`operators::brand_color`) rather than the DB column, which is unseeded
    // (all default grey) for the RDS-loaded operators.
    Ok(rows
        .into_iter()
        .map(|r| {
            let name = r.name.unwrap_or_else(|| "—".into());
            let brand = crate::ingestion::operators::brand_color(&name).to_string();
            OperatorHighlight {
                name,
                brand,
                on_time_pct: r.on_time_pct.unwrap_or(0.0),
                journeys: r.journeys,
            }
        })
        .collect())
}

async fn query_replay_trains(db: &Db, limit: i64) -> anyhow::Result<Vec<ReplayTrain>> {
    // Only replay services where the model made a *non-trivial* call
    // (`predicted_delay_mins <> 0`). Showing the bulk of services — where the
    // prediction was 0 — makes the board read as "it just guesses on-time". This
    // filters on prediction activity, NOT on accuracy, so the predicted→actual
    // outcome shown is whatever really happened (honest).
    let rows = sqlx::query_as::<_, ReplayRow>(
        r#"
        SELECT
            o.rid, o.uid,
            op.name                AS operator,
            o.origin_crs,
            o.destination_crs,
            o.scheduled_departure,
            o.predicted_delay_mins,
            o.final_delay_mins
        FROM prediction_outcomes o
        LEFT JOIN services  s  ON s.uid = o.uid
        LEFT JOIN operators op ON op.toc = s.toc
        WHERE o.finalised_at IS NOT NULL
          AND o.final_delay_mins IS NOT NULL
          AND o.final_delay_mins BETWEEN -120 AND 600
          AND o.predicted_delay_mins <> 0
        ORDER BY o.scheduled_departure DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let operator = r.operator.unwrap_or_else(|| "—".into());
            let brand = crate::ingestion::operators::brand_color(&operator).to_string();
            ReplayTrain {
                rid: r.rid,
                operator,
                brand,
                label: r.uid,
                origin: r.origin_crs,
                dest: r.destination_crs.unwrap_or_else(|| "—".into()),
                scheduled: r.scheduled_departure.format("%H:%M").to_string(),
                predicted: r.predicted_delay_mins,
                actual: r.final_delay_mins,
            }
        })
        .collect())
}

/// Assemble the full DemoData from the live DB.
pub async fn gather_demo(db: &Db, days: u32) -> anyhow::Result<DemoData> {
    let days_i = days as i32;
    let generated_at = Utc::now().format("%d %b %Y %H:%M UTC").to_string();

    let (kpis, operators, replay_trains) = tokio::try_join!(
        query_kpis(db, days_i),
        query_operator_highlights(db, days_i),
        query_replay_trains(db, 64),
    )?;

    // Destination arrival + recovery reliability from the journeys table
    // (journey_metrics takes hours, the demo window is days).
    let journey = crate::db::overview::journey_metrics(db, days_i * 24).await?;

    // More journeys on screen for a livelier board (8 tracking / 4 just-settled).
    let frames = build_frames(&replay_trains, 8, 4);

    Ok(DemoData {
        generated_at,
        hero_number: human_count(kpis.total_observations),
        hero_observations: kpis.total_observations,
        services_count: kpis.total_services,
        services_label: human_count(kpis.total_services),
        on_time_pct: kpis.on_time_pct,
        mae_mins: kpis.mae_mins,
        within_5_pct: kpis.within_5_pct,
        arrival_on_time_pct: journey.arrival_on_time_pct,
        recovered_pct: journey.recovered_pct,
        operators,
        frames,
    })
}

/// Query the DB, render the demo page, and write it to `output_path`.
pub async fn export_demo(db: &Db, output_path: &Path, days: u32) -> anyhow::Result<()> {
    let data = gather_demo(db, days).await?;
    let html = render_demo_html(&data)?;

    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, &html)?;

    println!(
        "Demo site exported: {} observations, {} replay frames → {}",
        data.hero_observations,
        data.frames.len(),
        output_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_card_has_no_actual_key() {
        let c = TrackCard {
            rid: "r1".into(), operator: "GWR".into(), brand: "#0a493e".into(),
            label: "1A23".into(), origin: "PAD".into(), dest: "BRI".into(),
            scheduled: "09:15".into(), predicted: 4,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("predicted").is_some());
        assert!(v.get("actual").is_none(), "tracking card must not expose an actual");
    }

    #[test]
    fn settled_card_exposes_predicted_actual_delta() {
        let c = SettledCard {
            rid: "r1".into(), operator: "GWR".into(), brand: "#0a493e".into(),
            label: "1A23".into(), origin: "PAD".into(), dest: "BRI".into(),
            predicted: 4, actual: 9, delta: 5,
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["predicted"], 4);
        assert_eq!(v["actual"], 9);
        assert_eq!(v["delta"], 5);
    }

    fn sample(n: usize) -> Vec<ReplayTrain> {
        (0..n).map(|k| ReplayTrain {
            rid: format!("r{k}"), operator: "GWR".into(), brand: "#0a493e".into(),
            label: format!("1A0{k}"), origin: "PAD".into(), dest: "BRI".into(),
            scheduled: "09:15".into(), predicted: 3, actual: 8,
        }).collect()
    }

    #[test]
    fn frames_progress_from_tracking_to_settled() {
        let trains = sample(3);
        let frames = build_frames(&trains, 2, 2);
        assert_eq!(frames.len(), 4); // i = 0..=3, none empty
        // First frame: nothing settled, trains tracking.
        assert_eq!(frames[0].settled.len(), 0);
        assert_eq!(frames[0].tracking.len(), 2);
        // Last frame: nothing tracking, recent trains settled (most recent first).
        assert_eq!(frames[3].tracking.len(), 0);
        assert_eq!(frames[3].settled[0].rid, "r2");
        // r0 is tracking at frame 0 and settled by frame 1.
        assert!(frames[0].tracking.iter().any(|c| c.rid == "r0"));
        assert!(frames[1].settled.iter().any(|c| c.rid == "r0"));
    }

    #[test]
    fn settled_delta_is_absolute_error() {
        let frames = build_frames(&sample(1), 1, 1);
        let last = frames.last().unwrap();
        assert_eq!(last.settled[0].delta, 5); // |8 - 3|
    }

    #[test]
    fn empty_input_yields_no_frames() {
        assert!(build_frames(&[], 4, 4).is_empty());
    }

    fn demo_fixture() -> DemoData {
        DemoData {
            generated_at: "06 Jun 2026 10:00 UTC".into(),
            hero_number: "2.5M+".into(),
            hero_observations: 2_546_226,
            services_count: 74_000,
            services_label: "74K+".into(),
            on_time_pct: Some(91.4),
            mae_mins: Some(3.2),
            within_5_pct: Some(78.0),
            arrival_on_time_pct: Some(88.0),
            recovered_pct: Some(41.0),
            operators: vec![],
            frames: build_frames(&sample(2), 2, 2),
        }
    }

    #[test]
    fn render_replaces_placeholder_and_keeps_anchors() {
        let html = render_demo_html(&demo_fixture()).unwrap();
        assert!(!html.contains("__DEMO_DATA__"), "placeholder must be replaced");
        assert!(html.contains(r#"id="beat-replay""#));
        assert!(html.contains("2.5M+") || html.contains("2546226"));
        assert!(!html.contains("/* __DEMO_JS__ */"), "JS marker must be replaced");
        assert!(html.contains("IntersectionObserver"), "inlined JS must be present");
    }

    #[test]
    fn human_count_formats_compactly() {
        assert_eq!(human_count(2_546_226), "2.5M+");
        assert_eq!(human_count(74_000), "74K+");
        assert_eq!(human_count(512), "512");
    }
}

//! `/map` live dashboard page + `/ui/map/snapshot` feed.
//!
//! Reuses the offline map renderer (`export::map`) driven by the live tracking
//! registry and recently-settled trains — the same GeoJSON, D3, and JS renderer
//! that power the offline export, inlined via `include_str!` so there is a single
//! source of truth with no static duplication.
//!
//! ## Endpoints
//! - `GET /map` — server-rendered command-centre shell (maud); KPIs are
//!   server-filled from `journey_metrics`; dots + banners refresh every 20 s.
//! - `GET /ui/map/snapshot` — JSON feed: coordinate-attached [`MapTrain`]s drawn
//!   from both the in-memory tracking registry and recently-settled outcomes.
//!   TIPLOCs without known coordinates are silently omitted.

use axum::extract::State;
use axum::response::Json;
use maud::{html, Markup, PreEscaped};
use serde::Serialize;

use crate::api::AppState;
use crate::cache::{location_coords, location_names};
use crate::db::{overview, predictions};
use crate::export::map::MapTrain;
use crate::frontend::layout::{base, NavPage};

// ---------------------------------------------------------------------------
// Assets inlined once at compile time — shared with `export::map`, no duplication
// ---------------------------------------------------------------------------

const D3: &str = include_str!("../export/assets/d3.v7.min.js");
const GB_OUTLINE: &str = include_str!("../export/assets/gb_outline.geojson");
const GB_RAIL: &str = include_str!("../export/assets/gb_rail.geojson");
const MAP_JS: &str = include_str!("../export/map_render.js");

// ---------------------------------------------------------------------------
// Snapshot response type
// ---------------------------------------------------------------------------

/// JSON payload for `/ui/map/snapshot`, polled by the live map page every 20 s.
#[derive(Serialize)]
pub struct MapSnapshot {
    /// Unix timestamp (seconds) at which this snapshot was assembled.
    t: i64,
    /// Placeable trains (tracking + recently settled), each with resolved coords.
    trains: Vec<MapTrain>,
}

// ---------------------------------------------------------------------------
// Pure coordinate-attaching helper — unit-testable without AppState
// ---------------------------------------------------------------------------

/// Neutral brand colour used for tracking trains whose operator is not yet known.
const NEUTRAL: &str = "#64748b";

/// Attempt to attach origin/destination coordinates to a tracking train.
///
/// Returns `None` when either TIPLOC has no known coordinates — those trains are
/// omitted from the map rather than plotted at (0, 0).
///
/// `coords()` returns `(lat, lon)` (WGS84); the map and GeoJSON expect `[lon, lat]`.
pub fn tracking_to_map_train(
    rid: &str,
    uid: Option<&str>,
    origin_tip: &str,
    dest_tip: &str,
    scheduled_hhmm: &str,
    predicted_delay_mins: i32,
) -> Option<MapTrain> {
    let (olat, olon) = location_coords::coords(origin_tip)?;
    let (dlat, dlon) = location_coords::coords(dest_tip)?;
    Some(MapTrain {
        label: uid.unwrap_or(rid).to_string(),
        operator: "—".to_string(),
        brand: NEUTRAL.to_string(),
        origin: location_names::name_or_code(origin_tip).to_string(),
        dest: location_names::name_or_code(dest_tip).to_string(),
        scheduled: scheduled_hhmm.to_string(),
        predicted: predicted_delay_mins,
        // Not yet settled — show prediction as the "actual" too so the colour
        // reflects the expected state at arrival, not a misleading 0.
        actual: predicted_delay_mins,
        delta: 0,
        o: [olon, olat],
        d: [dlon, dlat],
    })
}

// ---------------------------------------------------------------------------
// Snapshot handler
// ---------------------------------------------------------------------------

/// `GET /ui/map/snapshot` — coordinate-attached trains from the live feed.
///
/// Combines the in-memory tracking registry (active, predicted) with the most
/// recent settled outcomes from the DB. Trains whose TIPLOCs cannot be resolved
/// to coordinates are silently dropped — never faked.
pub async fn map_snapshot(State(state): State<AppState>) -> Json<MapSnapshot> {
    let tracking = state.registry.tracking_board(24).await;
    let settled = predictions::recent_settled(&state.db, 24)
        .await
        .unwrap_or_default();

    let mut trains: Vec<MapTrain> = Vec::new();

    // --- active tracking trains ---
    for t in tracking {
        let (Some(o_tip), Some(d_tip)) = (
            t.origin_crs.as_deref(),
            t.destination_crs.as_deref(),
        ) else {
            continue;
        };
        if let Some(mt) = tracking_to_map_train(
            &t.rid,
            t.uid.as_deref(),
            o_tip,
            d_tip,
            &t.scheduled_departure.format("%H:%M").to_string(),
            t.predicted_delay_mins,
        ) {
            trains.push(mt);
        }
    }

    // --- recently settled trains (predicted vs actual) ---
    for s in settled {
        let Some(d_tip) = s.destination_crs.as_deref() else {
            continue;
        };
        let (Some((olat, olon)), Some((dlat, dlon))) = (
            location_coords::coords(&s.origin_crs),
            location_coords::coords(d_tip),
        ) else {
            continue;
        };
        let operator = s.operator.unwrap_or_else(|| "—".to_string());
        trains.push(MapTrain {
            brand: crate::ingestion::operators::brand_color(&operator).to_string(),
            label: s.uid,
            operator,
            origin: location_names::name_or_code(&s.origin_crs).to_string(),
            dest: location_names::name_or_code(d_tip).to_string(),
            scheduled: String::new(),
            predicted: s.predicted_delay_mins,
            actual: s.final_delay_mins,
            delta: (s.final_delay_mins - s.predicted_delay_mins).abs(),
            o: [olon, olat],
            d: [dlon, dlat],
        });
    }

    Json(MapSnapshot {
        t: chrono::Utc::now().timestamp(),
        trains,
    })
}

// ---------------------------------------------------------------------------
// Page handler
// ---------------------------------------------------------------------------

/// `GET /map` — the live delay map command-centre.
///
/// Server-renders the KPI strip from `journey_metrics` (so those cards populate
/// instantly, before the first JSON poll). The GB map is driven by `map_render.js`
/// (inlined), which polls `/ui/map/snapshot` every 20 seconds.
pub async fn map_page(State(state): State<AppState>) -> Markup {
    let jm = overview::journey_metrics(&state.db, 24)
        .await
        .unwrap_or_default();
    let pct =
        |v: Option<f64>| v.map(|x| format!("{x:.0}%")).unwrap_or_else(|| "—".to_string());

    // Escape `</` in the GeoJSON blobs so they can't break out of a <script> tag.
    let outline_safe = GB_OUTLINE.replace("</", "<\\/");
    let rail_safe = GB_RAIL.replace("</", "<\\/");

    let body = html! {
        style {
r#"
.map-page { display:flex; flex-direction:column; gap:0; }
.map-heading { display:flex; align-items:center; gap:12px; padding:18px 0 14px; border-bottom:1px solid var(--border); margin-bottom:0; }
.map-heading h1 { font:700 18px var(--font-mono); letter-spacing:.01em; }
.map-heading .live-pill { display:flex; align-items:center; gap:6px; font:600 10px var(--font-mono); color:var(--ok); text-transform:uppercase; letter-spacing:.06em; }
.map-heading .live-pill .dot { width:7px; height:7px; border-radius:50%; background:var(--ok); animation:live-pulse 2s infinite; }
.cc-body { display:grid; grid-template-columns:230px 1fr 220px; gap:0; flex:1; min-height:580px; margin-top:0; border:1px solid var(--border); border-radius:var(--r-md); overflow:hidden; }
@media (max-width:960px) { .cc-body { grid-template-columns:1fr; } }
.cc-rail { padding:14px; overflow:auto; background:var(--surface); }
.cc-rail.left { border-right:1px solid var(--border); }
.cc-rail.right { border-left:1px solid var(--border); background:var(--surface-2); }
.cc-rail-head { font:700 10px var(--font-mono); text-transform:uppercase; letter-spacing:.08em; color:var(--text-dim); display:flex; align-items:center; gap:6px; margin:4px 0 10px; }
.cc-rail-head::before { content:""; width:7px; height:7px; border-radius:50%; background:var(--text-dim); }
.cc-rail-head.track::before { background:var(--accent); }
.cc-rail-head.settled::before { background:var(--ok); }
.cc-map { position:relative; background:linear-gradient(180deg, #0d1f38 0%, #091627 100%); min-height:560px; }
#map-svg { position:absolute; inset:0; width:100%; height:100%; cursor:grab; }
#map-svg:active { cursor:grabbing; }
.map-zoom { position:absolute; left:50%; transform:translateX(-50%); bottom:14px; display:flex; align-items:center; gap:9px; background:rgba(8,20,36,.74); border:1px solid rgba(127,178,232,.32); border-radius:22px; padding:7px 14px; backdrop-filter:blur(6px); z-index:6; }
.map-zoom input { width:170px; accent-color:#7fb2e8; cursor:pointer; }
.map-zoom b { font:700 14px var(--font-mono); color:#9ecbf2; width:12px; text-align:center; user-select:none; }
.map-zoom .lbl { font:600 9px var(--font-mono); color:#6f8fb4; text-transform:uppercase; letter-spacing:.06em; }
.kcard { background:var(--surface); border:1px solid var(--border); border-radius:var(--r-sm); padding:11px 13px; margin-bottom:8px; }
.kcard .kn { font:800 22px var(--font-sans); color:var(--accent); }
.kcard .kl { font:600 9px var(--font-mono); color:var(--text-dim); text-transform:uppercase; letter-spacing:.05em; margin-top:2px; }
.map-info { font-size:11.5px; color:var(--text-dim); line-height:1.55; margin-top:12px; }
.map-info b { color:var(--text); }
.map-legend { display:flex; align-items:center; gap:16px; flex-wrap:wrap; padding:9px 14px; border-top:1px solid var(--border); background:var(--surface); font:600 9.5px var(--font-mono); color:var(--text-dim); }
.lg { display:flex; align-items:center; gap:5px; }
.lg b { width:9px; height:9px; border-radius:50%; display:inline-block; }
.mini { display:grid; grid-template-columns:1fr auto; gap:1px 8px; border:1px solid var(--border); border-left:3px solid var(--c,#64748b); border-radius:var(--r-sm); padding:6px 9px; margin-bottom:6px; }
.mini .hc { font:700 11px var(--font-mono); }
.mini .rt { grid-column:1; font:600 9.5px var(--font-mono); color:var(--text-dim); }
.mini .pa { grid-column:2; grid-row:1/3; align-self:center; font:700 11px var(--font-mono); text-align:right; }
.mini .pa small { display:block; font:700 6.5px var(--font-mono); color:var(--text-faint); text-transform:uppercase; }
.map-empty { color:var(--text-dim); font:600 11px var(--font-mono); padding:8px 0; }
"#
        }

        div .map-page {
            // ── Heading row ──────────────────────────────────────────────
            div .map-heading {
                h1 { "Live delay map · Great Britain" }
                div .live-pill {
                    span .dot {}
                    span { "live" }
                }
            }

            // ── Command-centre body ──────────────────────────────────────
            div .cc-body {
                // Left rail — tracking + settled banners (populated by map_render.js)
                aside .cc-rail.left {
                    div .cc-rail-head.track { "Tracking" }
                    div # "rail-track" { p .map-empty { "Connecting…" } }
                    div .cc-rail-head.settled { "Just settled" }
                    div # "rail-settled" { p .map-empty { "—" } }
                }

                // Centre — map canvas + zoom slider
                main .cc-map {
                    svg # "map-svg" {}
                    div .map-zoom {
                        span .lbl { "zoom" }
                        b { "−" }
                        input # "map-zoom" type="range" min="1" max="50" step="0.5" value="10" {}
                        b { "+" }
                    }
                }

                // Right rail — server-rendered KPIs + info
                aside .cc-rail.right {
                    div .cc-rail-head { "The numbers" }
                    div .kcard {
                        div .kn { (pct(jm.arrival_on_time_pct)) }
                        div .kl { "arrive ≤5 min · rolling 24 h" }
                    }
                    div .kcard {
                        div .kn { (pct(jm.recovered_pct)) }
                        div .kl { "delay recovered en route" }
                    }
                    div .map-info {
                        b { "What you're seeing: " }
                        "every dot is a real train, coloured green (on time), "
                        "amber (slight delay), or red (late). Dots glide origin → "
                        "destination; near arrival the colour flips to actual delay. "
                        "The left panel mirrors predicted → actual outcomes."
                    }
                    div .map-info style="margin-top:16px;font-size:10.5px;" {
                        "Map © Crown copyright/ONS (OGL) · rail © OpenStreetMap (ODbL) · D3 (ISC)"
                    }
                }
            }

            // ── Legend strip ─────────────────────────────────────────────
            div .map-legend {
                span .lg { b style="background:#34d399" {} "on time" }
                span .lg { b style="background:#f2c14e" {} "slight delay" }
                span .lg { b style="background:#f04545" {} "late" }
            }
        }

        // ── Inline assets (compile-time, single source of truth) ─────────
        // 1. Seed the global window vars before the renderer runs.
        script {
            (PreEscaped(format!(
                "window.GB_OUTLINE={outline};window.GB_RAIL={rail};window.MAP={{trains:[]}};",
                outline = outline_safe,
                rail = rail_safe,
            )))
        }
        // 2. D3 v7 — required by map_render.js.
        script { (PreEscaped(D3)) }
        // 3. Renderer — exposes RailPredictMap.init().
        script { (PreEscaped(MAP_JS)) }
        // 4. Polling driver — fetch snapshot every 20 s and re-init the renderer.
        script {
            (PreEscaped(r#"
(function(){
  async function refresh(){
    try {
      var r = await fetch('/ui/map/snapshot', {cache:'no-store'});
      var d = await r.json();
      window.MAP = d;
      window.RailPredictMap.init();
    } catch(e){}
  }
  refresh();
  setInterval(refresh, 20000);
})();
"#))
        }
    };

    base("Map", NavPage::Map, body)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Tracking trains with both TIPLOCs resolvable produce a MapTrain whose
    /// `o`/`d` coordinates are `[lon, lat]` (GeoJSON order) in a valid GB box.
    #[test]
    fn tracking_to_map_train_both_resolve() {
        let mt = tracking_to_map_train(
            "202406010001",
            Some("C12345"),
            "WATRLMN",
            "GLGC",
            "09:15",
            3,
        )
        .expect("both TIPLOCs should resolve");

        // o/d = [lon, lat]; GB lon in -8..2, lat in 49..61
        assert!(
            (-8.0_f64..2.0).contains(&mt.o[0]) && (49.0_f64..61.0).contains(&mt.o[1]),
            "origin [lon,lat] not in GB box: {:?}",
            mt.o
        );
        assert!(
            (-8.0_f64..2.0).contains(&mt.d[0]) && (49.0_f64..61.0).contains(&mt.d[1]),
            "dest [lon,lat] not in GB box: {:?}",
            mt.d
        );

        assert_eq!(mt.label, "C12345");
        assert_eq!(mt.predicted, 3);
        // For a tracking (not yet settled) train, actual == predicted, delta == 0
        assert_eq!(mt.actual, 3);
        assert_eq!(mt.delta, 0);
    }

    /// A train with an unknown origin TIPLOC is dropped (returns None).
    #[test]
    fn tracking_to_map_train_unknown_origin_is_dropped() {
        assert!(
            tracking_to_map_train("R1", None, "ZZZZZZZ", "WATRLMN", "10:00", 0).is_none(),
            "unknown origin should return None"
        );
    }

    /// A train with an unknown destination TIPLOC is dropped (returns None).
    #[test]
    fn tracking_to_map_train_unknown_dest_is_dropped() {
        assert!(
            tracking_to_map_train("R1", None, "WATRLMN", "ZZZZZZZ", "10:00", 0).is_none(),
            "unknown destination should return None"
        );
    }

    /// Sanity-check that the coord lookup used by the snapshot handler compiles and
    /// resolves a well-known TIPLOC — ensures the `location_coords` import is wired.
    #[test]
    fn location_coords_resolves_waterloo() {
        assert!(
            location_coords::coords("WATRLMN").is_some(),
            "WATRLMN should have coordinates"
        );
    }

    /// `MapTrain` serialises to JSON containing the expected field names.
    #[test]
    fn map_train_roundtrips_json() {
        let mt = tracking_to_map_train("R1", Some("X99"), "WATRLMN", "GLGC", "08:00", -2)
            .expect("should resolve");
        let json = serde_json::to_string(&mt).expect("should serialise");
        assert!(json.contains("\"label\""));
        assert!(json.contains("\"o\""));
        assert!(json.contains("\"d\""));
        assert!(json.contains("X99"));
    }
}

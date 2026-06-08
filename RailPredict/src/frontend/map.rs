//! `/map` live dashboard page + `/ui/map/snapshot` feed.
//!
//! Reuses the offline map renderer (`export::map`) driven by the live tracking
//! registry and recently-settled trains — the same baked basemap snapshot and JS
//! renderer that power the offline export, inlined via `include_str!` so there is
//! a single source of truth with no static duplication.
//!
//! ## Endpoints
//! - `GET /map` — server-rendered command-centre shell (maud); KPIs are
//!   server-filled from `journey_metrics`; dots + banners refresh every 20 s.
//! - `GET /ui/map/snapshot` — JSON feed: coordinate-attached [`MapTrain`]s drawn
//!   from both the in-memory tracking registry and recently-settled outcomes.
//!   TIPLOCs without known coordinates are silently omitted.

use axum::extract::State;
use axum::response::{Html, Json};
use maud::{html, Markup, PreEscaped};
use serde::Serialize;

use crate::api::types::ApiError;
use crate::api::AppState;
use crate::cache::{location_coords, location_names, LiveService};
use crate::db::overview;
use crate::export::map::MapTrain;
use crate::frontend::layout::{base, NavPage};

// ---------------------------------------------------------------------------
// Assets inlined once at compile time — shared with `export::map`, no duplication
// ---------------------------------------------------------------------------

const MAP_B64: &str = include_str!("../export/assets/gb_map.b64");
const MAP_BOUNDS: &str = include_str!("../export/assets/gb_map_bounds.json");
const STATIONS: &str = include_str!("../export/assets/stations.json");
const EDGES: &str = include_str!("../export/assets/edges.json");
const JOURNEYS: &str = include_str!("../export/assets/journeys.json");
const MAP_JS: &str = include_str!("../export/map_render.js");

// ---------------------------------------------------------------------------
// Snapshot response type
// ---------------------------------------------------------------------------

/// JSON payload for `/ui/map/snapshot`, polled by the live map page every 20 s.
#[derive(Serialize)]
pub struct MapSnapshot {
    /// Unix timestamp (seconds) at which this snapshot was assembled.
    t: i64,
    /// Live, route-positioned services currently running on the network.
    trains: Vec<LiveService>,
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

/// `GET /ui/map/snapshot` — live, route-positioned services for the map.
///
/// Each currently-running service is turned into a route of `[lon,lat]` calling
/// points plus origin-departure / destination-arrival timing, so the renderer can
/// glide a node along it in real time. Stops without known coordinates are
/// dropped — never faked. See [`crate::cache::TrainRegistry::live_services`].
pub async fn map_snapshot(State(state): State<AppState>) -> Json<MapSnapshot> {
    let trains = state.registry.live_services().await;
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

    let body = html! {
        style {
r#"
.map-page { display:flex; flex-direction:column; gap:0; }
.map-heading { display:flex; align-items:center; gap:10px; padding:18px 0 14px; border-bottom:1px solid var(--border); margin-bottom:0; }
.map-heading h1 { font:700 16px var(--font-mono); letter-spacing:.01em; color:var(--text); }
.map-heading .map-sub { font:600 11px var(--font-mono); color:var(--text-dim); text-transform:uppercase; letter-spacing:.07em; }
.map-heading .map-sub::before { content:"·"; margin-right:8px; color:var(--border); }
.map-heading .live-pill { display:inline-flex; align-items:center; gap:7px; padding:4px 11px 4px 9px; border-radius:999px; background:rgba(52,211,153,.12); border:1px solid rgba(52,211,153,.34); font:700 10.5px var(--font-mono); color:var(--ok); text-transform:uppercase; letter-spacing:.08em; }
.map-heading .live-pill .dot { width:7px; height:7px; border-radius:50%; background:var(--ok); box-shadow:0 0 7px var(--ok); animation:live-pulse 2s infinite; }
.map-heading .live-pill .lp-count { color:var(--text); opacity:.92; letter-spacing:.02em; }
.map-heading .live-pill .lp-count:not(:empty)::before { content:"·"; margin:0 6px 0 1px; color:rgba(52,211,153,.55); }
.cc-body { display:grid; grid-template-columns:230px 1fr 220px; gap:0; flex:1; min-height:580px; margin-top:0; border:1px solid var(--border); border-radius:var(--r-md); overflow:hidden; }
@media (max-width:960px) { .cc-body { grid-template-columns:1fr; } }
.cc-rail { padding:14px; overflow:auto; background:var(--surface); }
.cc-rail.left { border-right:1px solid var(--border); }
.cc-rail.right { border-left:1px solid var(--border); background:var(--surface-2); }
.cc-rail-head { font:700 10px var(--font-mono); text-transform:uppercase; letter-spacing:.08em; color:var(--text-dim); display:flex; align-items:center; gap:6px; margin:4px 0 10px; }
.cc-rail-head::before { content:""; width:7px; height:7px; border-radius:50%; background:var(--text-dim); }
.cc-rail-head.track::before { background:var(--accent); }
.cc-rail-head.settled::before { background:var(--ok); }
.cc-map { position:relative; background:#1b1d22; min-height:560px; }
#map-svg { position:absolute; inset:0; width:100%; height:100%; cursor:grab; }
#map-svg:active { cursor:grabbing; }
.map-fs { position:absolute; top:12px; right:12px; z-index:6; width:30px; height:30px; display:flex; align-items:center; justify-content:center; background:rgba(8,20,36,.72); border:1px solid rgba(127,178,232,.32); border-radius:7px; color:#9ec3ef; font-size:15px; cursor:pointer; backdrop-filter:blur(6px); }
.map-fs:hover { background:rgba(20,40,68,.9); color:#fff; }
.map-hint { position:absolute; left:12px; bottom:10px; z-index:6; font:600 9px var(--font-mono); color:#7f93ad; letter-spacing:.04em; text-transform:uppercase; pointer-events:none; text-shadow:0 1px 3px #000; }
.map-clock-wrap { position:absolute; top:12px; left:50%; transform:translateX(-50%); z-index:6; display:flex; flex-direction:column; align-items:center; gap:1px; background:rgba(8,20,36,.74); border:1px solid rgba(127,178,232,.32); border-radius:9px; padding:5px 16px; backdrop-filter:blur(6px); }
.map-clock-wrap .lbl { font:700 7.5px var(--font-mono); color:#7f93ad; letter-spacing:.1em; text-transform:uppercase; }
#map-clock { font:800 18px var(--font-mono); color:#cfe6ff; letter-spacing:.03em; line-height:1.1; }
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
/* ── Full-bleed: break out of the 860px content column and fill the whole
   viewport below the 54px sticky nav, edge to edge. ── */
main:has(.map-page) { max-width:none; margin:0; padding:0; }
.map-page { height:calc(100dvh - 54px); }
.map-heading { padding:14px 20px 12px; }
.cc-body { border-left:none; border-right:none; border-radius:0; min-height:0; }
/* The map cell is a <div> (not <main>), but reset defensively: the global
   `main { max-width:860px; margin:0 auto; padding:… }` rule must never reach it.
   With all of its children absolutely positioned, auto side-margins would
   otherwise shrink it to just its padding — the "tiny centre strip" bug. */
.cc-map { max-width:none; margin:0; padding:0; min-width:0; }
.map-legend { padding-left:20px; padding-right:20px; }
@media (max-width:960px) { .map-page { height:auto; } .cc-body { min-height:560px; } }
"#
        }

        div .map-page {
            // ── Heading row ──────────────────────────────────────────────
            div .map-heading {
                h1 { "Live delay map" }
                span .map-sub { "Great Britain" }
                div .live-pill {
                    span .dot {}
                    "Live"
                    span # "map-count" .lp-count {}
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

                // Centre — map canvas + controls
                div .cc-map {
                    svg # "map-svg" {}
                    div .map-clock-wrap {
                        span .lbl { "Live · now" }
                        b # "map-clock" { "--:--" }
                    }
                    button # "map-fs" .map-fs title="Fullscreen" aria-label="Fullscreen" { "⛶" }
                    span .map-hint { "scroll to zoom · drag to pan" }
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
                        "every train currently running on the GB network, live. Grey shows the "
                        "network, brighter where busier; each bright node is a real service gliding "
                        "along its route in real time, coloured by its delay (green/amber/red). "
                        "Busy through the day, quiet overnight. Scroll to zoom; drag to pan; ⛶ fullscreen."
                    }
                    div .map-info style="margin-top:16px;font-size:10.5px;" {
                        "Map © OpenStreetMap contributors © CARTO"
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
        // 1. Seed the basemap snapshot + its bounds before the renderer runs.
        script {
            (PreEscaped(format!(
                "window.MAP_IMG=\"data:image/png;base64,{b64}\";window.MAP_BOUNDS={bounds};window.MAP_STATIONS={stations};window.MAP_EDGES={edges};window.MAP_JOURNEYS={journeys};window.MAP={{trains:[]}};",
                b64 = MAP_B64.trim(),
                bounds = MAP_BOUNDS.trim(),
                stations = STATIONS.trim(),
                edges = EDGES.trim(),
                journeys = JOURNEYS.trim(),
            )))
        }
        // 2. Renderer — exposes RailPredictMap.init().
        script { (PreEscaped(MAP_JS)) }
        // 3. Polling driver — fetch snapshot every 20 s and re-init the renderer.
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

/// `GET /demo` — the self-contained "RailPredict OS" desktop, rendered live from
/// the same generator that produces the offline `docs/os.html`. A full-screen
/// standalone page (its own chrome), so it isn't wrapped in the dashboard nav.
pub async fn demo_page(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    let data = crate::export::os::gather_os(&state.db, 7)
        .await
        .map_err(|e| ApiError::internal(format!("demo data gather failed: {e}")))?;
    let html = crate::export::os::render_os_html(&data)
        .map_err(|e| ApiError::internal(format!("demo render failed: {e}")))?;
    Ok(Html(html))
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

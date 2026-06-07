//! Self-contained offline "command-centre" map (`export-map` CLI subcommand).
//!
//! Bakes a real OSM/CARTO GB map snapshot (raster, base64-inlined) with
//! delay-coloured train dots placed by lat/lon, plus predicted→actual banners and
//! KPIs, into one offline file. Train positions come from `cache::location_coords`;
//! unresolved TIPLOCs are omitted (never faked). Mirrors `export::demo`.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;

use crate::cache::location_coords;
use crate::db::Db;
use crate::export::demo::human_count;

const TEMPLATE: &str = include_str!("map_template.html");
const MAP_B64: &str = include_str!("assets/gb_map.b64");
const MAP_BOUNDS: &str = include_str!("assets/gb_map_bounds.json");
const STATIONS: &str = include_str!("assets/stations.json");
const EDGES: &str = include_str!("assets/edges.json");
const MAP_JS: &str = include_str!("map_render.js");

/// One train placed on the map. `o`/`d` are `[lon, lat]` (GeoJSON/d3 order).
#[derive(Serialize, Clone, Debug)]
pub struct MapTrain {
    pub label: String,
    pub operator: String,
    pub brand: String,
    pub origin: String,
    pub dest: String,
    pub scheduled: String,
    pub predicted: i32,
    pub actual: i32,
    pub delta: i32,
    pub o: [f64; 2],
    pub d: [f64; 2],
}

#[derive(Serialize, Clone, Debug)]
pub struct MapData {
    pub generated_at: String,
    pub hero_number: String,
    pub on_time_pct: Option<f64>,
    pub mae_mins: Option<f64>,
    pub within_5_pct: Option<f64>,
    pub services_label: String,
    pub arrival_on_time_pct: Option<f64>,
    pub recovered_pct: Option<f64>,
    pub trains: Vec<MapTrain>,
}

/// A raw replay train before coordinates are attached (plain — unit-testable).
pub struct RawTrain {
    pub uid: String,
    pub operator: Option<String>,
    pub origin: String,
    pub dest: Option<String>,
    pub scheduled: String,
    pub predicted: i32,
    pub actual: i32,
}

/// Attach coordinates to a raw train, or drop it (origin/dest unresolved, no dest).
/// `cache::location_coords` returns (lat, lon); the map needs `[lon, lat]`.
fn to_map_train(r: &RawTrain) -> Option<MapTrain> {
    let dest = r.dest.as_deref()?;
    let (olat, olon) = location_coords::coords(&r.origin)?;
    let (dlat, dlon) = location_coords::coords(dest)?;
    let operator = r.operator.clone().unwrap_or_else(|| "—".into());
    let brand = crate::ingestion::operators::brand_color(&operator).to_string();
    Some(MapTrain {
        label: r.uid.clone(),
        operator,
        brand,
        origin: r.origin.clone(),
        dest: dest.to_string(),
        scheduled: r.scheduled.clone(),
        predicted: r.predicted,
        actual: r.actual,
        delta: (r.actual - r.predicted).abs(),
        o: [olon, olat],
        d: [dlon, dlat],
    })
}

#[derive(FromRow)]
struct KpiRow {
    total_observations: i64,
    total_services: i64,
    on_time_pct: Option<f64>,
    mae_mins: Option<f64>,
    within_5_pct: Option<f64>,
}

#[derive(FromRow)]
struct MapRow {
    uid: String,
    operator: Option<String>,
    origin_crs: String,
    destination_crs: Option<String>,
    scheduled_departure: DateTime<Utc>,
    predicted_delay_mins: i32,
    final_delay_mins: i32,
}

async fn query_kpis(db: &Db, days: i32) -> anyhow::Result<KpiRow> {
    Ok(sqlx::query_as::<_, KpiRow>(
        r#"
        SELECT
            COUNT(*)                                                        AS total_observations,
            COUNT(DISTINCT uid || '|' || origin_crs)                        AS total_services,
            (AVG(CASE WHEN delay_mins <= 5 THEN 1.0 ELSE 0.0 END) * 100)::float8 AS on_time_pct,
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
    .await?)
}

async fn query_map_rows(db: &Db, limit: i64) -> anyhow::Result<Vec<MapRow>> {
    Ok(sqlx::query_as::<_, MapRow>(
        r#"
        SELECT o.uid, op.name AS operator, o.origin_crs, o.destination_crs,
               o.scheduled_departure, o.predicted_delay_mins, o.final_delay_mins
        FROM prediction_outcomes o
        LEFT JOIN services  s  ON s.uid = o.uid
        LEFT JOIN operators op ON op.toc = s.toc
        WHERE o.finalised_at IS NOT NULL
          AND o.final_delay_mins IS NOT NULL
          AND o.final_delay_mins BETWEEN -120 AND 600
          AND o.predicted_delay_mins <> 0
          AND o.destination_crs IS NOT NULL
        ORDER BY o.scheduled_departure DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(db)
    .await?)
}

/// Assemble MapData from the live DB. Caps at 80 placeable trains for a lively
/// but legible map.
pub async fn gather_map(db: &Db, days: u32) -> anyhow::Result<MapData> {
    let days_i = days as i32;
    let generated_at = Utc::now().format("%d %b %Y %H:%M UTC").to_string();
    let kpis = query_kpis(db, days_i).await?;
    let journey = crate::db::overview::journey_metrics(db, days_i * 24).await?;
    let rows = query_map_rows(db, 200).await?;

    let trains: Vec<MapTrain> = rows
        .into_iter()
        .map(|r| RawTrain {
            uid: r.uid,
            operator: r.operator,
            origin: r.origin_crs,
            dest: r.destination_crs,
            scheduled: r.scheduled_departure.format("%H:%M").to_string(),
            predicted: r.predicted_delay_mins,
            actual: r.final_delay_mins,
        })
        .filter_map(|r| to_map_train(&r))
        .take(80)
        .collect();

    Ok(MapData {
        generated_at,
        hero_number: human_count(kpis.total_observations),
        on_time_pct: kpis.on_time_pct,
        mae_mins: kpis.mae_mins,
        within_5_pct: kpis.within_5_pct,
        services_label: human_count(kpis.total_services),
        arrival_on_time_pct: journey.arrival_on_time_pct,
        recovered_pct: journey.recovered_pct,
        trains,
    })
}

/// Inline the basemap snapshot (base64), its Web-Mercator bounds, the renderer JS,
/// and the data into the template. Escapes `</` in JSON literals so a stray
/// `</script>` can't break out.
pub fn render_map_html(data: &MapData) -> anyhow::Result<String> {
    let json = serde_json::to_string(data)?.replace("</", "<\\/");
    let html = TEMPLATE
        .replace("__MAP_B64__", MAP_B64.trim())
        .replace("__MAP_BOUNDS__", MAP_BOUNDS.trim())
        .replace("__STATIONS__", STATIONS.trim())
        .replace("__EDGES__", EDGES.trim())
        .replace("__MAP_DATA__", &json)
        .replace("/* __MAP_JS__ */", MAP_JS);
    Ok(html)
}

/// Query the DB, render the map page, write it to `output_path`.
pub async fn export_map(db: &Db, output_path: &Path, days: u32) -> anyhow::Result<()> {
    let data = gather_map(db, days).await?;
    let html = render_map_html(&data)?;
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, &html)?;
    println!(
        "Map exported: {} trains plotted → {}",
        data.trains.len(),
        output_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(origin: &str, dest: Option<&str>) -> RawTrain {
        RawTrain {
            uid: "C12345".into(),
            operator: Some("Great Western Railway".into()),
            origin: origin.into(),
            dest: dest.map(|d| d.into()),
            scheduled: "09:15".into(),
            predicted: 3,
            actual: 8,
        }
    }

    #[test]
    fn resolvable_train_gets_lonlat_and_delta() {
        let t = to_map_train(&raw("WATRLMN", Some("GLGC"))).expect("both resolve");
        // o/d are [lon, lat]; GB lon in -8..2, lat in 49..61.
        assert!((-8.0..2.0).contains(&t.o[0]) && (49.0..61.0).contains(&t.o[1]));
        assert!((-8.0..2.0).contains(&t.d[0]) && (49.0..61.0).contains(&t.d[1]));
        assert_eq!(t.delta, 5); // |8 - 3|
        assert_eq!(t.brand, "#0a493e"); // GWR
    }

    #[test]
    fn unresolved_or_missing_dest_is_dropped() {
        assert!(to_map_train(&raw("WATRLMN", Some("ZZZZZZZ"))).is_none());
        assert!(to_map_train(&raw("ZZZZZZZ", Some("GLGC"))).is_none());
        assert!(to_map_train(&raw("WATRLMN", None)).is_none());
    }

    #[test]
    fn render_replaces_all_placeholders() {
        let data = MapData {
            generated_at: "x".into(), hero_number: "1M+".into(), on_time_pct: Some(90.0),
            mae_mins: Some(1.0), within_5_pct: Some(80.0), services_label: "50K+".into(),
            arrival_on_time_pct: Some(88.0), recovered_pct: Some(40.0),
            trains: vec![to_map_train(&raw("WATRLMN", Some("GLGC"))).unwrap()],
        };
        let html = render_map_html(&data).unwrap();
        for tok in ["__MAP_B64__", "__MAP_BOUNDS__", "__STATIONS__", "__EDGES__", "__MAP_DATA__", "/* __MAP_JS__ */"] {
            assert!(!html.contains(tok), "placeholder {tok} not replaced");
        }
        assert!(html.contains("data:image/png;base64,")); // basemap snapshot inlined
        assert!(html.contains("RailPredictMap")); // renderer inlined
    }
}

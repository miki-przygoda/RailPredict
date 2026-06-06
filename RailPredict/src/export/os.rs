//! Self-contained offline "RailPredict OS" desktop (`export-os` CLI subcommand).
//!
//! Bakes the macOS-style shell (`os_template.html` + `os_shell.js` + `os_apps.js`)
//! with all apps' data into one offline file. `window.MAP` carries map trains (Map
//! app), operators + replay frames (the other apps), and shared KPIs — composed
//! by reusing `export::demo::gather_demo` and `export::map::gather_map`.

use std::path::Path;

use serde::Serialize;

use crate::db::Db;

const TEMPLATE: &str = include_str!("os_template.html");
const D3: &str = include_str!("assets/d3.v7.min.js");
const GB_OUTLINE: &str = include_str!("assets/gb_outline.geojson");
const GB_RAIL: &str = include_str!("assets/gb_rail.geojson");
const MAP_JS: &str = include_str!("map_render.js");
const OS_JS: &str = include_str!("os_shell.js");
const APPS_JS: &str = include_str!("os_apps.js");

/// The single payload baked into `window.MAP` for the whole desktop.
#[derive(Serialize, Clone, Debug)]
pub struct OsData {
    pub generated_at: String,
    pub hero_number: String,
    pub hero_observations: i64,
    pub services_label: String,
    pub on_time_pct: Option<f64>,
    pub mae_mins: Option<f64>,
    pub within_5_pct: Option<f64>,
    pub arrival_on_time_pct: Option<f64>,
    pub recovered_pct: Option<f64>,
    pub operators: Vec<crate::export::demo::OperatorHighlight>,
    pub frames: Vec<crate::export::demo::Frame>,
    pub trains: Vec<crate::export::map::MapTrain>,
}

/// Compose the OS payload from the demo gather (KPIs + operators + replay frames)
/// and the map gather (coordinate-attached trains).
pub async fn gather_os(db: &Db, days: u32) -> anyhow::Result<OsData> {
    let demo = crate::export::demo::gather_demo(db, days).await?;
    let trains = crate::export::map::gather_map(db, days).await?.trains;
    Ok(OsData {
        generated_at: demo.generated_at,
        hero_number: demo.hero_number,
        hero_observations: demo.hero_observations,
        services_label: demo.services_label,
        on_time_pct: demo.on_time_pct,
        mae_mins: demo.mae_mins,
        within_5_pct: demo.within_5_pct,
        arrival_on_time_pct: demo.arrival_on_time_pct,
        recovered_pct: demo.recovered_pct,
        operators: demo.operators,
        frames: demo.frames,
        trains,
    })
}

/// Inline D3, both GeoJSON layers, the map renderer, the app renderers, the OS
/// shell, and the data into the desktop template. Escapes `</` in JSON literals.
pub fn render_os_html(data: &OsData) -> anyhow::Result<String> {
    let json = serde_json::to_string(data)?.replace("</", "<\\/");
    let outline = GB_OUTLINE.replace("</", "<\\/");
    let railjson = GB_RAIL.replace("</", "<\\/");
    let html = TEMPLATE
        .replace("__D3__", D3)
        .replace("__GB_OUTLINE__", &outline)
        .replace("__GB_RAIL__", &railjson)
        .replace("__MAP_DATA__", &json)
        .replace("/* __MAP_JS__ */", MAP_JS)
        .replace("/* __APPS_JS__ */", APPS_JS)
        .replace("/* __OS_JS__ */", OS_JS);
    Ok(html)
}

/// Query the DB, render the desktop, write it to `output_path`.
pub async fn export_os(db: &Db, output_path: &Path, days: u32) -> anyhow::Result<()> {
    let data = gather_os(db, days).await?;
    let html = render_os_html(&data)?;
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, &html)?;
    println!(
        "RailPredict OS exported: {} trains, {} operators, {} replay frames → {}",
        data.trains.len(),
        data.operators.len(),
        data.frames.len(),
        output_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> OsData {
        OsData {
            generated_at: "x".into(), hero_number: "1M+".into(), hero_observations: 1_000_000,
            services_label: "50K+".into(), on_time_pct: Some(90.0), mae_mins: Some(1.0),
            within_5_pct: Some(80.0), arrival_on_time_pct: Some(88.0), recovered_pct: Some(40.0),
            operators: vec![], frames: vec![], trains: vec![],
        }
    }

    #[test]
    fn render_replaces_all_placeholders_and_inlines_apps() {
        let html = render_os_html(&fixture()).unwrap();
        for tok in ["__D3__", "__GB_OUTLINE__", "__GB_RAIL__", "__MAP_DATA__", "/* __MAP_JS__ */", "/* __APPS_JS__ */", "/* __OS_JS__ */"] {
            assert!(!html.contains(tok), "placeholder {tok} not replaced");
        }
        assert!(html.contains("geoConicConformal"), "d3 inlined");
        assert!(html.contains("RailPredictMap"), "map renderer inlined");
        assert!(html.contains("window.OsApps"), "app renderers inlined");
        assert!(html.contains("openApp"), "os shell inlined");
    }
}

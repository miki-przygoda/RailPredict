//! Self-contained offline "RailPredict OS" desktop (`export-os` CLI subcommand).
//!
//! Bakes the macOS-style shell (`os_template.html` + `os_shell.js`) with the Map
//! app — the shared D3 renderer (`map_render.js`), GB GeoJSON basemap, and the
//! `MapData` payload — into one offline file. Reuses `export::map::gather_map`.

use std::path::Path;

use crate::db::Db;
use crate::export::map::{gather_map, MapData};

const TEMPLATE: &str = include_str!("os_template.html");
const D3: &str = include_str!("assets/d3.v7.min.js");
const GB_OUTLINE: &str = include_str!("assets/gb_outline.geojson");
const GB_RAIL: &str = include_str!("assets/gb_rail.geojson");
const MAP_JS: &str = include_str!("map_render.js");
const OS_JS: &str = include_str!("os_shell.js");

/// Inline D3, both GeoJSON layers, the map renderer, the OS shell, and the data
/// into the desktop template. Escapes `</` in JSON literals so a stray
/// `</script>` can't break out of the host script.
pub fn render_os_html(data: &MapData) -> anyhow::Result<String> {
    let json = serde_json::to_string(data)?.replace("</", "<\\/");
    let outline = GB_OUTLINE.replace("</", "<\\/");
    let railjson = GB_RAIL.replace("</", "<\\/");
    let html = TEMPLATE
        .replace("__D3__", D3)
        .replace("__GB_OUTLINE__", &outline)
        .replace("__GB_RAIL__", &railjson)
        .replace("__MAP_DATA__", &json)
        .replace("/* __MAP_JS__ */", MAP_JS)
        .replace("/* __OS_JS__ */", OS_JS);
    Ok(html)
}

/// Query the DB, render the desktop, write it to `output_path`.
pub async fn export_os(db: &Db, output_path: &Path, days: u32) -> anyhow::Result<()> {
    let data = gather_map(db, days).await?;
    let html = render_os_html(&data)?;
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output_path, &html)?;
    println!(
        "RailPredict OS exported: {} trains on the map → {}",
        data.trains.len(),
        output_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::map::MapTrain;

    fn fixture() -> MapData {
        MapData {
            generated_at: "x".into(),
            hero_number: "1M+".into(),
            on_time_pct: Some(90.0),
            mae_mins: Some(1.0),
            within_5_pct: Some(80.0),
            services_label: "50K+".into(),
            arrival_on_time_pct: Some(88.0),
            recovered_pct: Some(40.0),
            trains: vec![MapTrain {
                label: "1A23".into(), operator: "GWR".into(), brand: "#0a493e".into(),
                origin: "PAD".into(), dest: "BRI".into(), scheduled: "09:15".into(),
                predicted: 3, actual: 8, delta: 5, o: [-0.17, 51.51], d: [-2.58, 51.45],
            }],
        }
    }

    #[test]
    fn render_replaces_all_placeholders_and_inlines_shell() {
        let html = render_os_html(&fixture()).unwrap();
        for tok in ["__D3__", "__GB_OUTLINE__", "__GB_RAIL__", "__MAP_DATA__", "/* __MAP_JS__ */", "/* __OS_JS__ */"] {
            assert!(!html.contains(tok), "placeholder {tok} not replaced");
        }
        assert!(html.contains("geoConicConformal"), "d3 inlined");
        assert!(html.contains("RailPredictMap"), "map renderer inlined");
        assert!(html.contains("openApp"), "os shell inlined");
        assert!(html.contains("id=\"app-map\""), "map app template present");
    }
}

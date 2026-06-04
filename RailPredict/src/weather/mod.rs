//! Weather integration: fetches wind speed from Open-Meteo every 10 minutes
//! and writes results into a shared VolatilityStore.

use std::sync::Arc;

use dashmap::DashMap;
use serde::Deserialize;

/// Maps route_id (a CRS code representing a route anchor point) to
/// current wind speed in mph. Written by the weather task; read by the
/// poll consumer when evaluating state promotions.
pub type VolatilityStore = Arc<DashMap<String, f32>>;

/// A (lat, lon, route_id) tuple defining a bounding-box centre to poll.
#[derive(Debug, Clone)]
pub struct WeatherAnchor {
    pub route_id: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoResponse {
    hourly: OpenMeteoHourly,
}

#[derive(Debug, Deserialize)]
struct OpenMeteoHourly {
    windspeed_10m: Vec<f32>,
}

/// Fetch current wind speed (mph) for a given lat/lon from Open-Meteo.
///
/// Open-Meteo returns values in km/h by default; pass `wind_speed_unit=mph` to get mph.
/// The hourly array covers the next 168 hours — index 0 is the current hour.
pub async fn fetch_wind_mph(
    client: &reqwest::Client,
    lat: f64,
    lon: f64,
) -> anyhow::Result<f32> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast\
         ?latitude={lat}&longitude={lon}\
         &hourly=windspeed_10m&wind_speed_unit=mph&forecast_days=1"
    );
    let resp: OpenMeteoResponse = client.get(&url).send().await?.json().await?;
    resp.hourly
        .windspeed_10m
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Open-Meteo returned empty windspeed array"))
}

/// Run the weather polling loop: refresh every 10 minutes, write into the store.
///
/// Spawned as a background task. If a fetch fails, log and continue — stale
/// data is better than a panic. Wind speed stays at the last known value.
pub async fn run_weather_task(
    store: VolatilityStore,
    anchors: Vec<WeatherAnchor>,
    client: reqwest::Client,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(600));
    loop {
        interval.tick().await;
        for anchor in &anchors {
            match fetch_wind_mph(&client, anchor.lat, anchor.lon).await {
                Ok(mph) => {
                    tracing::debug!(route = %anchor.route_id, wind_mph = mph, "weather update");
                    store.insert(anchor.route_id.clone(), mph);
                }
                Err(e) => {
                    tracing::warn!(route = %anchor.route_id, error = %e, "weather fetch failed");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_weather_anchors_valid() {
        let raw = "LDS:53.796:-1.548,MAN:53.488:-2.242";
        let result = crate::config::parse_weather_anchors(raw);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "LDS");
        assert!((result[0].1 - 53.796).abs() < 1e-6);
        assert!((result[0].2 - (-1.548)).abs() < 1e-6);
        assert_eq!(result[1].0, "MAN");
    }

    #[test]
    fn parse_weather_anchors_malformed_skipped() {
        let raw = "LDS:bad:data,MAN:53.488:-2.242,KGX";
        let result = crate::config::parse_weather_anchors(raw);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "MAN");
    }

    #[test]
    fn parse_weather_anchors_empty() {
        let result = crate::config::parse_weather_anchors("");
        assert!(result.is_empty());
    }
}

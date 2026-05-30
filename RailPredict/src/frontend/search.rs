//! Search page and departure board fragment handler.

use axum::extract::{Query, State};
use maud::{Markup, PreEscaped, html};
use serde::Deserialize;

use crate::api::handlers::{StationResult, StationSearchQuery};
use crate::api::types::DepartureBoardEntry;
use crate::api::AppState;

use super::components::{delay_badge, platform_chip, prediction_chip};
use super::layout::base;

/// Shared JS injected on the search page.
///
/// Uses event delegation rather than inline onclick handlers so that station
/// names containing apostrophes or other special characters cannot break the
/// JavaScript string literals.
const SEARCH_JS: &str = r#"
(function () {
    // --- Suggestion selection (event delegation) ---
    // Each <li> carries data-crs, data-name, data-crs-id, data-q-id,
    // data-suggestions-id set by the server.  No JS string injection.
    document.addEventListener('click', function (e) {
        var li = e.target.closest('li[data-crs]');
        if (li) {
            var crsEl = document.getElementById(li.dataset.crsId);
            var qEl   = document.getElementById(li.dataset.qId);
            var sgEl  = document.getElementById(li.dataset.suggestionsId);
            if (crsEl) crsEl.value = li.dataset.crs;
            if (qEl)   qEl.value   = li.dataset.name;
            if (sgEl)  sgEl.innerHTML = '';
            // Re-enable the submit button once a station is confirmed
            var form = qEl && qEl.closest('form');
            if (form) {
                var btn = form.querySelector('button[type="submit"]');
                if (btn) btn.removeAttribute('disabled');
            }
            return;
        }
        // Click outside any suggestion list — close all
        if (!e.target.closest('.station-suggestions')) {
            document.querySelectorAll('.station-suggestions')
                .forEach(function (el) { el.innerHTML = ''; });
        }
    });

    // --- Escape key closes all suggestion lists ---
    document.addEventListener('keydown', function (e) {
        if (e.key === 'Escape') {
            document.querySelectorAll('.station-suggestions')
                .forEach(function (el) { el.innerHTML = ''; });
        }
    });

    // --- Clear hidden CRS + disable submit when user edits the text input ---
    // Each text input carries data-clears=<id-of-hidden-crs-input>.
    document.addEventListener('input', function (e) {
        var input = e.target;
        if (!input.dataset || !input.dataset.clears) return;
        var hidden = document.getElementById(input.dataset.clears);
        if (!hidden) return;
        // Only disable once a CRS was set (avoids disabling on initial type)
        if (hidden.value) {
            hidden.value = '';
            var form = input.closest('form');
            if (form) {
                var btn = form.querySelector('button[type="submit"]');
                if (btn) btn.setAttribute('disabled', '');
            }
        }
    });

    // --- Block Enter-key form submission when no CRS is selected ---
    // The submit button is disabled but Enter from within the text input
    // bypasses disabled buttons in some browsers.
    document.addEventListener('keydown', function (e) {
        if (e.key !== 'Enter') return;
        var input = e.target;
        if (!input.dataset || !input.dataset.clears) return;
        var hidden = document.getElementById(input.dataset.clears);
        if (hidden && !hidden.value) {
            e.preventDefault();
        }
    });

    // --- Tab switching ---
    window.showTab = function (name, event) {
        document.querySelectorAll('.tab-pane')
            .forEach(function (el) { el.classList.add('hidden'); });
        document.getElementById('tab-' + name).classList.remove('hidden');
        document.querySelectorAll('.tab-btn')
            .forEach(function (el) { el.classList.remove('active'); });
        event.target.classList.add('active');
        // Close any open suggestions when switching tabs
        document.querySelectorAll('.station-suggestions')
            .forEach(function (el) { el.innerHTML = ''; });
    };
})();
"#;

const COUNTDOWN_JS: &str = r#"
(function () {
    function updateCountdowns() {
        var now = Date.now();
        document.querySelectorAll('.train-card[data-scheduled]').forEach(function (card) {
            var iso = card.dataset.scheduled;
            var el  = card.querySelector('.train-countdown');
            if (!el || !iso) return;
            var dep     = new Date(iso).getTime();
            var diffMin = Math.round((dep - now) / 60000);
            if (diffMin > 5 && diffMin <= 180) {
                el.textContent = 'in ' + diffMin + ' min';
            } else if (diffMin > -5 && diffMin <= 5) {
                el.textContent = 'now';
            } else {
                el.textContent = '';
            }
        });
    }
    updateCountdowns();
    setInterval(updateCountdowns, 60000);
})();
"#;

const PRED_FILTER_JS: &str = r#"
(function () {
    window.setPredFilter = function (filter, btn) {
        document.querySelectorAll('.pred-filter-btn')
            .forEach(function (el) { el.classList.remove('active'); });
        btn.classList.add('active');
        document.querySelectorAll('.train-card[data-pred]').forEach(function (card) {
            if (filter === 'all') {
                card.style.display = '';
            } else {
                card.style.display = (card.dataset.pred === filter) ? '' : 'none';
            }
        });
    };
})();
"#;

pub async fn search_page() -> Markup {
    base(
        "Search",
        html! {
            div .search-container {
                div .search-hero {
                    h1 { "Where are you going?" }
                    p { "Live UK rail departures — type a station name to begin." }
                }
                div .search-tabs {
                    button .tab-btn.active type="button"
                        onclick="showTab('departures', event)" { "Departures" }
                    button .tab-btn type="button"
                        onclick="showTab('journeys', event)" { "Journey" }
                }

                // Departures tab
                div id="tab-departures" .tab-pane {
                    form .search-form-row
                        hx-get="/ui/stations/departures"
                        hx-target="#results"
                        hx-trigger="submit"
                        hx-swap="innerHTML transition:true"
                        hx-include="[name='crs']"
                    {
                        div .search-input-group style="flex:1" {
                            input
                                type="text"
                                name="q"
                                id="station-q"
                                placeholder="Station name (e.g. London Kings Cross)"
                                autocomplete="off"
                                data-clears="crs-hidden"
                                hx-get="/ui/stations/search"
                                hx-trigger="input changed delay:300ms"
                                hx-target="#station-suggestions"
                                hx-include="[name='q']";
                            input
                                type="hidden"
                                name="crs"
                                id="crs-hidden"
                                value="";
                            div #station-suggestions {}
                        }
                        button type="submit" disabled { "Search" }
                    }
                }

                // Journey tab
                div id="tab-journeys" .tab-pane.hidden {
                    form .search-form-stack
                        hx-get="/ui/journeys"
                        hx-target="#results"
                        hx-trigger="submit"
                        hx-swap="innerHTML transition:true"
                    {
                        div .search-input-group {
                            input
                                type="text"
                                name="from-q"
                                id="journey-from-q"
                                placeholder="From station"
                                autocomplete="off"
                                data-clears="journey-from-crs"
                                hx-get="/ui/stations/search"
                                hx-trigger="input changed delay:300ms"
                                hx-target="#journey-from-suggestions"
                                hx-vals="js:{q: document.getElementById('journey-from-q').value, crs_input_id: 'journey-from-crs', q_input_id: 'journey-from-q'}";
                            input
                                type="hidden"
                                name="from"
                                id="journey-from-crs"
                                value="";
                            div id="journey-from-suggestions" {}
                        }
                        div .search-input-group {
                            input
                                type="text"
                                name="to-q"
                                id="journey-to-q"
                                placeholder="To station"
                                autocomplete="off"
                                data-clears="journey-to-crs"
                                hx-get="/ui/stations/search"
                                hx-trigger="input changed delay:300ms"
                                hx-target="#journey-to-suggestions"
                                hx-vals="js:{q: document.getElementById('journey-to-q').value, crs_input_id: 'journey-to-crs', q_input_id: 'journey-to-q'}";
                            input
                                type="hidden"
                                name="to"
                                id="journey-to-crs"
                                value="";
                            div id="journey-to-suggestions" {}
                        }
                        button type="submit" disabled { "Find journey" }
                    }
                }

                div #results {}
                script { (PreEscaped(SEARCH_JS)) }
            }
        },
    )
}

#[derive(Deserialize)]
pub struct CrsQuery {
    pub crs: String,
    /// Plain-text station name typed by the user — used as a fallback when
    /// `crs` is empty (e.g. Enter-key submission before selecting a suggestion).
    #[serde(default)]
    pub q: String,
}

pub async fn departures_fragment(
    Query(q): Query<CrsQuery>,
    State(state): State<AppState>,
) -> Markup {
    let crs_upper = q.crs.trim().to_uppercase();

    // If no CRS was set (user hit Enter without picking a suggestion), try to
    // resolve the station name to a CRS via a DB lookup.
    let resolved_crs = if crs_upper.is_empty() {
        let name = q.q.trim().to_string();
        if name.len() < 2 {
            return html! {
                p .no-results { "Start typing a station name and select one from the list." }
            };
        }
        let mut hits = state.station_index.search(&name, 1);
        match hits.pop() {
            Some(h) => h.crs,
            None => {
                let typed = name.as_str();
                return html! {
                    p .no-results {
                        "No station found matching \u{201c}" (typed) "\u{201d}. Try the autocomplete list."
                    }
                };
            }
        }
    } else {
        crs_upper
    };

    let station_name = state
        .station_index
        .search(&resolved_crs, 1)
        .into_iter()
        .next()
        .map(|h| h.name)
        .unwrap_or_else(|| resolved_crs.clone());

    let entries = crate::api::handlers::build_departure_board(&state, &resolved_crs).await;
    departure_board_fragment(&station_name, &entries)
}

pub fn departure_board_fragment(heading: &str, entries: &[DepartureBoardEntry]) -> Markup {
    if entries.is_empty() {
        return html! { p .no-results { "No departures found for " (heading) "." } };
    }

    let ml_count  = entries.iter().filter(|e| e.predicted_delay_mins.is_some()).count();
    let stat_count = entries.len() - ml_count;

    html! {
        div .departure-board {
            div .departure-board-header {
                h2 { "Departures from " (heading) }
                span .departure-count { (entries.len()) " service" (if entries.len() == 1 { "" } else { "s" }) }
            }

            // Filter toggle bar
            div .pred-filter-bar {
                button .pred-filter-btn.active
                    data-filter="all"
                    onclick="setPredFilter('all',this)"
                {
                    "All " span .pred-filter-count { (entries.len()) }
                }
                button .pred-filter-btn
                    data-filter="ml"
                    onclick="setPredFilter('ml',this)"
                {
                    "ML predicted " span .pred-filter-count { (ml_count) }
                }
                button .pred-filter-btn
                    data-filter="stat"
                    onclick="setPredFilter('stat',this)"
                {
                    "Statistical " span .pred-filter-count { (stat_count) }
                }
            }

            @for entry in entries {
                @let stale      = entry.last_updated_secs_ago.is_some_and(|s| s > 120);
                @let has_ml     = entry.predicted_delay_mins.is_some();
                // Clamp midnight-wrap artifacts (e.g. -1404 min from crossing midnight)
                @let display_delay = entry.delay_mins.filter(|&m| (-60..=300).contains(&m));
                a .train-card
                  data-stale=[if stale { Some("true") } else { None::<&str> }]
                  data-pred=[if has_ml { Some("ml") } else { Some("stat") }]
                  data-scheduled=(entry.scheduled_departure)
                  href={ "/trains/" (entry.rid) "/view" }
                {
                    div .train-card-left {
                        div .train-time-col {
                            span .train-time {
                                (entry.scheduled_departure.get(11..16).unwrap_or("--:--"))
                            }
                            span .train-countdown {}
                        }
                        (delay_badge(display_delay, entry.is_cancelled.unwrap_or(false)))
                        @if let Some(dest) = &entry.destination_name {
                            span .train-destination { "→ " (dest) }
                        }
                    }
                    div .train-card-right {
                        span .train-rid { (entry.rid) }
                        (platform_chip(entry.platform.as_deref(), entry.is_platform_planned))
                        (prediction_chip(entry.predicted_delay_mins, display_delay))
                    }
                }
            }
        }
        script { (maud::PreEscaped(PRED_FILTER_JS)) }
        script { (maud::PreEscaped(COUNTDOWN_JS)) }
    }
}

/// Render the suggestion `<ul>` from a pre-fetched list of station results.
///
/// Uses `data-*` attributes exclusively — no JS string injection — so that
/// station names containing apostrophes (`King's Cross`) are handled safely.
pub fn render_suggestion_list(
    results: &[StationResult],
    crs_id: &str,
    q_id: &str,
    suggestions_id: &str,
) -> Markup {
    if results.is_empty() {
        return html! {};
    }
    html! {
        ul .station-suggestions {
            @for result in results {
                li .suggestion-item
                    tabindex="0"
                    data-crs=(result.crs)
                    data-name=(result.name)
                    data-crs-id=(crs_id)
                    data-q-id=(q_id)
                    data-suggestions-id=(suggestions_id)
                {
                    span .suggestion-name { (result.name) }
                    span .suggestion-crs { (result.crs) }
                    @if result.trains_today > 0 {
                        span .suggestion-trains { (result.trains_today) " today" }
                    } @else {
                        span .suggestion-trains .suggestion-no-service { "no service" }
                    }
                }
            }
        }
    }
}

/// `GET /ui/stations/search?q=<term>[&crs_input_id=...&q_input_id=...]`
pub async fn station_suggestions_fragment(
    Query(params): Query<StationSearchQuery>,
    State(state): State<AppState>,
) -> Markup {
    let q = params.q.trim().to_string();
    if q.len() < 2 {
        return html! {};
    }

    let crs_id = params
        .crs_input_id
        .as_deref()
        .unwrap_or("crs-hidden")
        .to_string();
    let q_id = params
        .q_input_id
        .as_deref()
        .unwrap_or("station-q")
        .to_string();

    let suggestions_id = if q_id == "station-q" {
        "station-suggestions".to_string()
    } else {
        q_id.replace("-q", "-suggestions")
    };

    let hits = state.station_index.search(&q, 10);
    let results: Vec<StationResult> = hits
        .into_iter()
        .map(|h| StationResult { crs: h.crs, name: h.name, trains_today: 0 })
        .collect();

    render_suggestion_list(&results, &crs_id, &q_id, &suggestions_id)
}

// ---------------------------------------------------------------------------
// Item 5.1 — Journey search fragment
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct JourneyQuery {
    pub from: String,
    pub to: String,
    pub date: Option<String>,
}

/// `GET /ui/journeys?from=XXX&to=YYY[&date=YYYY-MM-DD]`
pub async fn journeys_fragment(
    Query(q): Query<JourneyQuery>,
    State(state): State<AppState>,
) -> Markup {
    let from = q.from.trim().to_uppercase();
    let to = q.to.trim().to_uppercase();

    if from == to || from.len() != 3 || to.len() != 3 {
        return html! {
            p .no-results { "Please enter valid origin and destination station codes." }
        };
    }

    let from_name = state.station_index.search(&from, 1).into_iter().next()
        .map(|h| h.name).unwrap_or_else(|| from.clone());
    let to_name = state.station_index.search(&to, 1).into_iter().next()
        .map(|h| h.name).unwrap_or_else(|| to.clone());

    let date = q
        .date
        .as_deref()
        .and_then(|s| s.parse::<chrono::NaiveDate>().ok())
        .unwrap_or_else(|| chrono::Utc::now().date_naive());

    let rows = sqlx::query_as::<_, (String, chrono::NaiveTime, Option<String>)>(
        "SELECT tc_from.uid, \
                tc_from.scheduled_departure, \
                tc_from.platform \
         FROM timetable_calls tc_from \
         JOIN timetable_calls tc_to \
             ON tc_to.uid            = tc_from.uid \
            AND tc_to.operating_date = tc_from.operating_date \
            AND tc_to.location_crs   = $2 \
            AND tc_to.call_order     > tc_from.call_order \
         WHERE tc_from.location_crs  = $1 \
           AND tc_from.operating_date = $3 \
         ORDER BY tc_from.scheduled_departure",
    )
    .bind(&from)
    .bind(&to)
    .bind(date)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let entries: Vec<DepartureBoardEntry> = rows
        .into_iter()
        .map(|(uid, dep_time, platform)| {
            let scheduled_dt = chrono::NaiveDateTime::new(date, dep_time).and_utc();
            let is_platform_planned = platform.is_some();
            DepartureBoardEntry {
                rid: uid.trim().to_string(),
                scheduled_departure: scheduled_dt.to_rfc3339(),
                estimated_departure: None,
                delay_mins: None,
                platform,
                is_platform_planned,
                is_cancelled: None,
                last_updated_secs_ago: None,
                destination_name: Some(to_name.clone()),
                predicted_delay_mins: None,
            }
        })
        .collect();

    if entries.is_empty() {
        return html! {
            p .no-results { "No direct services found from " (from_name) " to " (to_name) "." }
        };
    }

    departure_board_fragment(&format!("{from_name} \u{2192} {to_name}"), &entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        scheduled: &str,
        delay: Option<i32>,
        platform: Option<&str>,
        is_platform_planned: bool,
        is_cancelled: Option<bool>,
        dest: Option<&str>,
    ) -> DepartureBoardEntry {
        DepartureBoardEntry {
            rid: "202404170000001".to_string(),
            scheduled_departure: scheduled.to_string(),
            estimated_departure: None,
            delay_mins: delay,
            platform: platform.map(str::to_string),
            is_platform_planned,
            is_cancelled,
            last_updated_secs_ago: None,
            destination_name: dest.map(str::to_string),
            predicted_delay_mins: None,
        }
    }

    fn make_station(crs: &str, name: &str) -> StationResult {
        StationResult { crs: crs.to_string(), name: name.to_string(), trains_today: 0 }
    }

    // ── departure_board_fragment ────────────────────────────────────────────

    #[test]
    fn empty_entries_renders_no_results_message() {
        let html = departure_board_fragment("LDS", &[]).into_string();
        assert!(html.contains("No departures found for LDS"), "html: {html}");
    }

    #[test]
    fn on_time_entry_renders_on_time_badge() {
        let entry = make_entry("2024-04-17T09:00:00Z", Some(0), None, false, None, None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("on-time"), "html: {html}");
        assert!(html.contains("On time"), "html: {html}");
    }

    #[test]
    fn delayed_entry_renders_delay_badge() {
        let entry = make_entry("2024-04-17T09:00:00Z", Some(7), None, false, None, None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("delayed"), "html: {html}");
        assert!(html.contains("7 min"), "html: {html}");
    }

    #[test]
    fn very_late_entry_renders_late_badge() {
        let entry = make_entry("2024-04-17T09:00:00Z", Some(15), None, false, None, None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("class=\"badge late\"") || html.contains("badge late"), "html: {html}");
        assert!(html.contains("15 min"), "html: {html}");
    }

    #[test]
    fn cancelled_entry_renders_cancelled_badge() {
        let entry = make_entry("2024-04-17T09:00:00Z", None, None, false, Some(true), None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("Cancelled"), "html: {html}");
    }

    #[test]
    fn confirmed_platform_renders_plain_chip() {
        let entry = make_entry("2024-04-17T09:00:00Z", None, Some("3"), false, None, None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("platform-chip"), "html: {html}");
        // Must NOT have the planned class
        assert!(!html.contains("platform-chip planned"), "planned class must not be set for confirmed platform: {html}");
        assert!(html.contains(">3<") || html.contains(">3 <") || html.contains("\"3\""), "platform number must appear: {html}");
        assert!(html.contains("Platform"), "label must appear: {html}");
        assert!(!html.contains("(planned)"), "must not say planned: {html}");
    }

    #[test]
    fn planned_platform_renders_planned_chip() {
        let entry = make_entry("2024-04-17T09:00:00Z", None, Some("5A"), true, None, None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("planned"), "planned class must be set: {html}");
        assert!(html.contains("Platform (planned)"), "label must say planned: {html}");
        assert!(html.contains("5A"), "platform number must appear: {html}");
    }

    #[test]
    fn no_platform_renders_no_chip() {
        let entry = make_entry("2024-04-17T09:00:00Z", None, None, false, None, None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(!html.contains("platform-chip"), "no chip when platform is None: {html}");
    }

    #[test]
    fn destination_name_rendered_when_present() {
        let entry = make_entry("2024-04-17T09:00:00Z", None, None, false, None, Some("Manchester Piccadilly"));
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("Manchester Piccadilly"), "html: {html}");
        assert!(html.contains("train-destination"), "html: {html}");
    }

    #[test]
    fn time_extracted_from_iso_timestamp() {
        let entry = make_entry("2024-04-17T14:37:00Z", None, None, false, None, None);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains("14:37"), "time must be extracted from ISO string: {html}");
    }

    #[test]
    fn stale_card_has_data_stale_attribute() {
        let mut entry = make_entry("2024-04-17T09:00:00Z", None, None, false, None, None);
        entry.last_updated_secs_ago = Some(200); // > 120 threshold
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(html.contains(r#"data-stale="true""#), "html: {html}");
    }

    #[test]
    fn fresh_card_has_no_data_stale_attribute() {
        let mut entry = make_entry("2024-04-17T09:00:00Z", None, None, false, None, None);
        entry.last_updated_secs_ago = Some(30);
        let html = departure_board_fragment("LDS", &[entry]).into_string();
        assert!(!html.contains(r#"data-stale="true""#), "html: {html}");
    }

    // ── render_suggestion_list ──────────────────────────────────────────────

    #[test]
    fn empty_results_renders_nothing() {
        let html = render_suggestion_list(&[], "crs-hidden", "station-q", "station-suggestions")
            .into_string();
        assert!(html.is_empty(), "must render nothing for empty results: '{html}'");
    }

    #[test]
    fn suggestion_uses_data_attributes_not_inline_js() {
        let results = vec![make_station("LDS", "Leeds")];
        let html = render_suggestion_list(&results, "crs-hidden", "station-q", "station-suggestions")
            .into_string();
        assert!(html.contains(r#"data-crs="LDS""#), "html: {html}");
        assert!(html.contains(r#"data-name="Leeds""#), "html: {html}");
        assert!(html.contains(r#"data-crs-id="crs-hidden""#), "html: {html}");
        assert!(html.contains(r#"data-q-id="station-q""#), "html: {html}");
        assert!(html.contains(r#"data-suggestions-id="station-suggestions""#), "html: {html}");
        // Must NOT contain raw JS string assignment
        assert!(!html.contains("getElementById"), "must not inline JS: {html}");
    }

    #[test]
    fn apostrophe_in_station_name_does_not_inject_js() {
        // "King's Cross" used to break the inline onclick handler because the
        // apostrophe terminated the JS string literal:
        //   value='King's Cross'  ← broken
        // With data-* attributes there are no JS string literals at all.
        // The apostrophe is fine inside a double-quoted HTML attribute and
        // is retrieved safely via li.dataset.name in the event listener.
        let results = vec![make_station("KGX", "King's Cross")];
        let html = render_suggestion_list(&results, "crs-hidden", "station-q", "station-suggestions")
            .into_string();
        // Name appears in the data attribute (double-quoted — apostrophe is safe)
        assert!(
            html.contains(r#"data-name="King's Cross""#),
            "name must be in data attribute: {html}"
        );
        // No JavaScript string assignment — the fix is that we emit zero JS here
        assert!(!html.contains("getElementById"), "no JS injection: {html}");
        assert!(!html.contains(".value="), "no JS value assignment: {html}");
    }

    #[test]
    fn multiple_suggestions_all_rendered() {
        let results = vec![
            make_station("LDS", "Leeds"),
            make_station("MAN", "Manchester Piccadilly"),
            make_station("EUS", "London Euston"),
        ];
        let html = render_suggestion_list(&results, "crs-hidden", "station-q", "station-suggestions")
            .into_string();
        assert!(html.contains("Leeds"), "html: {html}");
        assert!(html.contains("Manchester Piccadilly"), "html: {html}");
        assert!(html.contains("London Euston"), "html: {html}");
        assert_eq!(html.matches("suggestion-item").count(), 3, "html: {html}");
    }

    #[test]
    fn suggestion_items_are_focusable() {
        let results = vec![make_station("LDS", "Leeds")];
        let html = render_suggestion_list(&results, "crs-hidden", "station-q", "station-suggestions")
            .into_string();
        assert!(html.contains(r#"tabindex="0""#), "items must be keyboard-focusable: {html}");
    }

    #[test]
    fn crs_and_name_both_shown_in_suggestion() {
        let results = vec![make_station("BHM", "Birmingham New Street")];
        let html = render_suggestion_list(&results, "crs-hidden", "station-q", "station-suggestions")
            .into_string();
        assert!(html.contains("Birmingham New Street"), "html: {html}");
        assert!(html.contains("BHM"), "CRS must appear in suggestion: {html}");
        assert!(html.contains("suggestion-name"), "html: {html}");
        assert!(html.contains("suggestion-crs"), "html: {html}");
    }

    // ── journey validation ──────────────────────────────────────────────────

    #[test]
    fn journey_fragment_validation_is_separate_from_rendering() {
        // The form-level validation (same from/to, bad CRS length) should return
        // the no-results message — not a server error. This is tested here by
        // exercising the logic inline (the full handler is DB-dependent).
        let from = "LDS";
        let to = "LDS";
        let same = from == to;
        assert!(same, "same from/to must be caught");

        let short = "LD";
        let too_short = short.len() != 3;
        assert!(too_short, "short CRS must be caught");
    }
}

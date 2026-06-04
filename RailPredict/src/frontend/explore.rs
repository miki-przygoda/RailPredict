//! Query Explorer page — `/explore`.
//!
//! A guided "fill-in-the-gaps" data explorer: the admin composes a query from
//! whitelisted controls that read like a sentence, and the server runs a safe
//! parameterized query (`db::explore`) and renders a result table. Each control
//! change re-requests `/explore` via htmx, swaps the `#explore-results` panel
//! (`hx-select`), and pushes the URL — so any composed query is bookmarkable.
//! `?format=csv` returns the same result as a downloadable CSV.
//!
//! NOTE: functional, minimally-styled markup — slated for reskin in the UI overhaul
//! (see `docs/superpowers/specs/2026-06-04-visual-changes-plan.md`).

use axum::extract::{Query, RawQuery, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use maud::{html, Markup};
use serde::Deserialize;

use crate::api::AppState;
use crate::db::explore::{self, ExploreResult, ExploreSpec, RawExplore};
use crate::db::operators::{list_operators, Operator};

use super::layout::{base, NavPage};

/// Raw (untrusted) query-string params. Kept as strings so empty/garbage values
/// never fail to parse; validation happens in `ExploreSpec::from_raw`.
#[derive(Debug, Default, Deserialize)]
pub struct ExploreParams {
    pub subject: Option<String>,
    pub window: Option<String>,
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub from_hour: Option<String>,
    pub to_hour: Option<String>,
    pub weekdays: Option<String>,
    pub operator: Option<String>,
    pub origin: Option<String>,
    pub destination: Option<String>,
    pub min_delay: Option<String>,
    pub group: Option<String>,
    pub metric: Option<String>,
    pub limit: Option<String>,
    pub format: Option<String>,
}

impl ExploreParams {
    fn to_spec(&self) -> ExploreSpec {
        ExploreSpec::from_raw(RawExplore {
            subject: self.subject.as_deref(),
            window: self.window.as_deref(),
            from_date: self.from_date.as_deref(),
            to_date: self.to_date.as_deref(),
            from_hour: self.from_hour.as_deref().and_then(|s| s.parse().ok()),
            to_hour: self.to_hour.as_deref().and_then(|s| s.parse().ok()),
            weekdays: self.weekdays.as_deref(),
            operator: self.operator.as_deref(),
            origin: self.origin.as_deref(),
            destination: self.destination.as_deref(),
            min_delay: self.min_delay.as_deref().and_then(|s| s.parse().ok()),
            group: self.group.as_deref(),
            metric: self.metric.as_deref(),
            limit: self.limit.as_deref().and_then(|s| s.parse().ok()),
        })
    }
}

/// `GET /explore` — full page, or (with `?format=csv`) a CSV download.
pub async fn explore_page(
    State(state): State<AppState>,
    Query(params): Query<ExploreParams>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let spec = params.to_spec();
    let result = explore::run_explore(&state.db, &spec).await.ok();

    if params.format.as_deref() == Some("csv") {
        let csv = result.map(|r| r.to_csv()).unwrap_or_default();
        return (
            [
                (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"explore.csv\""),
            ],
            csv,
        )
            .into_response();
    }

    let operators = list_operators(&state.db).await.unwrap_or_default();
    let csv_href = match raw_query {
        Some(q) if !q.is_empty() => format!("/explore?{q}&format=csv"),
        _ => "/explore?format=csv".to_string(),
    };
    base(
        "Explore",
        NavPage::Explore,
        render(&params, &spec, &operators, result, &csv_href),
    )
    .into_response()
}

fn render(
    p: &ExploreParams,
    spec: &ExploreSpec,
    operators: &[Operator],
    result: Option<ExploreResult>,
    csv_href: &str,
) -> Markup {
    html! {
        div .explore {
            h1 { "Query Explorer" }
            p .explore-sub {
                "Compose a query from the options below — results update live. "
                "Example: avg delay for observations between hour 6 and 10, operator Heathrow Express, grouped by hour."
            }

            form .explore-form
                hx-get="/explore"
                hx-target="#explore-results"
                hx-select="#explore-results"
                hx-push-url="true"
                hx-trigger="change, input changed delay:400ms"
            {
                span .explore-word { "Show" }
                (select("metric", &[
                    ("list", "matching trains"), ("count", "count"),
                    ("avg_delay", "avg delay"), ("on_time_pct", "on-time %"),
                    ("p50", "median delay"), ("p90", "p90 delay"),
                    ("mae", "prediction MAE"), ("avg_confidence", "avg confidence"),
                ], spec.metric.key()))

                span .explore-word { "for" }
                (select("subject", &[
                    ("observations", "delay observations"),
                    ("predictions", "predictions"),
                    ("cancellations", "cancellations"),
                ], spec.subject.key()))

                span .explore-word { "between hour" }
                input .explore-num type="number" name="from_hour" min="0" max="23"
                    value=[p.from_hour.clone()] placeholder="0";
                span .explore-word { "and" }
                input .explore-num type="number" name="to_hour" min="0" max="23"
                    value=[p.to_hour.clone()] placeholder="23";

                span .explore-word { "operator" }
                select name="operator" {
                    option value="" { "any" }
                    @for op in operators {
                        option value=(op.toc) selected[p.operator.as_deref() == Some(op.toc.as_str())] {
                            (op.name)
                        }
                    }
                }

                span .explore-word { "from" }
                input .explore-crs type="text" name="origin" maxlength="3"
                    value=[p.origin.clone()] placeholder="CRS";
                span .explore-word { "to" }
                input .explore-crs type="text" name="destination" maxlength="3"
                    value=[p.destination.clone()] placeholder="CRS";

                span .explore-word { "delay ≥" }
                input .explore-num type="number" name="min_delay"
                    value=[p.min_delay.clone()] placeholder="any";

                span .explore-word { "over" }
                (select("window", &[
                    ("24h", "24h"), ("7d", "7 days"), ("30d", "30 days"), ("all", "all time"),
                ], p.window.as_deref().unwrap_or("7d")))
                span .explore-word { "or dates" }
                input .explore-date type="date" name="from_date" value=[p.from_date.clone()];
                span .explore-word { "to" }
                input .explore-date type="date" name="to_date" value=[p.to_date.clone()];

                span .explore-word { "grouped by" }
                (select("group", &[
                    ("none", "nothing"), ("operator", "operator"), ("origin", "origin"),
                    ("destination", "destination"), ("route", "route"), ("hour", "hour"),
                    ("weekday", "weekday"), ("day", "day"),
                ], spec.group_by.key()))
            }

            div .explore-toolbar {
                a .explore-csv href=(csv_href) { "Download CSV" }
            }

            div # "explore-results" .explore-results {
                (results_panel(result.as_ref()))
            }
        }
    }
}

/// A `<select name>` whose option matching `current` is pre-selected.
fn select(name: &str, options: &[(&str, &str)], current: &str) -> Markup {
    html! {
        select name=(name) {
            @for (value, label) in options {
                option value=(value) selected[*value == current] { (label) }
            }
        }
    }
}

fn results_panel(result: Option<&ExploreResult>) -> Markup {
    match result {
        None => html! {
            p .explore-empty { "Couldn't run that query. Adjust the options and try again." }
        },
        Some(res) if res.rows.is_empty() => html! {
            p .explore-empty { "No matching records in this range." }
        },
        Some(res) => html! {
            @if res.truncated {
                p .explore-note { "Showing the first " (res.rows.len()) " rows (capped)." }
            } @else {
                p .explore-note { (res.rows.len()) " row" (if res.rows.len() == 1 { "" } else { "s" }) }
            }
            table .explore-table {
                thead { tr { @for c in &res.columns { th { (c) } } } }
                tbody {
                    @for row in &res.rows {
                        tr { @for cell in row { td { (cell) } } }
                    }
                }
            }
        },
    }
}

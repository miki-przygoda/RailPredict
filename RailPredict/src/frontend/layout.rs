//! Base HTML chrome shared by every page.

use maud::{DOCTYPE, Markup, PreEscaped, html};

/// Which top-level page is active — drives nav highlighting.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NavPage {
    Dashboard,
    Departures,
    Predictions,
    Explore,
    DevConsole,
    None,
}

pub fn base(title: &str, active: NavPage, content: Markup) -> Markup {
    let link = |href: &str, label: &str, page: NavPage| {
        let is_active = page == active;
        html! {
            a href=(href) class=(if is_active { "nav-link active" } else { "nav-link" })
                aria-current=[is_active.then_some("page")] { (label) }
        }
    };
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — RailPredict" }
                link rel="preload" as="font" type="font/woff2" href="/static/fonts/IBMPlexMono-Regular.woff2" crossorigin;
                link rel="preload" as="font" type="font/woff2" href="/static/fonts/IBMPlexSans-Regular.woff2" crossorigin;
                link rel="stylesheet" href="/static/style.css";
                script src="https://unpkg.com/htmx.org@2.0.3" crossorigin="anonymous" {}
                script src="https://unpkg.com/htmx-ext-sse@2.2.2/sse.js" crossorigin="anonymous" {}
            }
            body {
                nav {
                    a .nav-brand href="/" {
                        span .nav-brand-dot {}
                        "RailPredict"
                    }
                    div .nav-links {
                        (link("/", "Dashboard", NavPage::Dashboard))
                        (link("/search", "Departures", NavPage::Departures))
                        (link("/predictions", "Predictions", NavPage::Predictions))
                        (link("/explore", "Explore", NavPage::Explore))
                        (link("/demo", "Dev Console", NavPage::DevConsole))
                    }
                }
                main { (content) }
                div # "stale-banner" .hidden ."warning-banner" {
                    svg .warn-icon width="14" height="14" viewBox="0 0 24 24" fill="none"
                        stroke="currentColor" stroke-width="2" aria-hidden="true" {
                        path d="M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" {}
                    }
                    " Live updates paused — showing last known state ("
                    span # "stale-timestamp" {}
                    ")"
                }
                script {
                    (PreEscaped(r#"
                        document.addEventListener('htmx:sseError', function() {
                            var banner = document.getElementById('stale-banner');
                            var ts = document.getElementById('stale-timestamp');
                            if (ts) ts.textContent = 'last seen ' + new Date().toLocaleTimeString();
                            if (banner) banner.classList.remove('hidden');
                            var live = document.getElementById('live-status');
                            if (live) live.classList.add('data-stale');
                        });
                        document.addEventListener('htmx:sseOpen', function() {
                            var banner = document.getElementById('stale-banner');
                            if (banner) banner.classList.add('hidden');
                            var live = document.getElementById('live-status');
                            if (live) live.classList.remove('data-stale');
                        });
                    "#))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NavPage, base};

    #[test]
    fn active_page_is_marked() {
        let html = base("Test", NavPage::Predictions, maud::html! { p { "x" } }).into_string();
        assert!(html.contains("aria-current=\"page\""), "marks active link: {html}");
        assert!(html.contains("Dashboard"), "has dashboard link");
        assert!(!html.contains('\u{26A0}'), "emoji replaced with svg");
    }
}

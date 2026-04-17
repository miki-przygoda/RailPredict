//! Base HTML chrome shared by every page.

use maud::{DOCTYPE, Markup, PreEscaped, html};

pub fn base(title: &str, content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) " — RailPredict" }
                link rel="stylesheet" href="/static/style.css";
                script src="https://unpkg.com/htmx.org@2.0.3" crossorigin="anonymous" {}
                script src="https://unpkg.com/htmx-ext-sse@2.2.2/sse.js" crossorigin="anonymous" {}
            }
            body {
                nav {
                    a href="/" { "RailPredict" }
                }
                main {
                    (content)
                }
                div # "stale-banner" .hidden ."warning-banner" {
                    "⚠ Live updates paused — showing last known state ("
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

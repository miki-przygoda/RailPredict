# RailPredict: High-Efficiency UK Rail Data Engine

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.2.0" -- 17/04/2026**

---

## The Problem

The Great British Railways (GBR) API is slow, rate-limited, and expensive to hit repeatedly. A naive rail app that calls the live API for every search query, every page load, and every status check will feel sluggish and will burn through its API quota the moment traffic spikes.

Most rail apps are dumb mirrors: ask GBR, show the result, repeat. RailPredict is built on the premise that the vast majority of the data a user needs can be served without ever touching GBR at all.

---

## The Idea: An Intelligent Buffer

RailPredict acts as a **shadow system** — a local layer that sits between users and GBR and tries to answer every query from its own knowledge before falling back to a live call. It does this by separating rail data into three tiers based on how "fresh" it actually needs to be:

**Tier A — Static data** (timetables, station names, base fares) changes once a week. Download it in bulk, store it locally, serve it for free.

**Tier B — Predictive data** (likely delay, typical platform, fare range) can be inferred from history. If the 08:01 from Leeds is late 85% of Mondays, say so — without calling GBR.

**Tier C — Transactional data** (exact live position, seat availability, final ticket lock) genuinely requires a live call. But this should only happen at the last possible moment: when the user is about to pay.

The goal is that by the time a user reaches checkout, the system has already pre-warmed the relevant data so the final live call is the only one that matters.

---

## How the System Stays Efficient

### Progressive Disclosure
Data is revealed in stages matched to what the user actually needs at each step:
- **Search results page:** Show last-known price and scheduled time (Tier A, zero cost).
- **Train detail page:** Show predicted delay based on local history (Tier B, local compute).
- **Checkout page:** Make the single live call to lock the ticket (Tier C, one call).

### The State Machine
Not every train needs the same attention. A train departing in three hours is irrelevant right now; a train departing in four minutes needs constant watching. RailPredict uses a state machine to assign each train a polling frequency appropriate to its urgency:

- **Dormant** — departure > 2 hours: no live calls, static data only.
- **Monitored** — 30–120 minutes out: poll every 10 minutes, build up delay probability.
- **Active** — 0–30 minutes out: poll every 30–60 seconds.
- **Critical** — under 5 minutes OR a disruption detected: real-time stream or 10-second polling.

External events can force an emergency promotion. A signal failure, a weather alert, or a social media spike about a specific route can push an entire corridor straight to Critical state, regardless of departure time.

### Request Coalescing
If 50 users are watching the same train, the system makes one outbound API call and fans the result to all 50 — not 50 separate calls.

### Darwin Push Port
Rather than polling GBR for updates, RailPredict subscribes to the **Darwin Push Port** (a STOMP-based firehose of every train movement in the UK). Incoming updates are stored in a fast in-memory cache. User queries are served from that cache — GBR is never touched for read-only requests.

### Circuit Breaker
If GBR starts returning errors (overloaded, rate-limited), the system automatically enters "Cache Only" mode and stops sending requests until GBR recovers. Users continue to see data; GBR is protected from additional load.

---

## Tech Stack

| Concern                | Tool                                                                   |
|:-----------------------|:-----------------------------------------------------------------------|
| Async runtime          | `tokio`                                                                |
| HTTP client (GBR REST) | `reqwest` (rustls-tls, no native-tls)                                  |
| Darwin firehose        | STOMP client (`tokio`-based)                                           |
| In-memory cache        | `dashmap` (concurrent)                                                 |
| Time handling          | `chrono`                                                               |
| XML parsing (Darwin)   | `quick-xml`                                                            |
| Serialisation          | `serde` + `serde_json`                                                 |
| Database               | `sqlx` 0.8 (Postgres, async, compile-time checked queries)             |
| HTTP framework         | `axum` 0.7                                                             |
| HTML templating        | `maud`                                                                 |
| Static assets          | `rust-embed`                                                           |
| Observability          | `tracing` + `tracing-subscriber` + `metrics` + `metrics-exporter-prometheus` |
| Rate limiting          | `tower_governor` (pending — tracked in ProductionHardening epic)       |
| CLI                    | `clap` 4 (derive)                                                      |
| Error handling         | `thiserror` (domain errors) + `anyhow` (app-level)                     |

---

## North Star

A rail application that feels **instant**. By the time the user has chosen a train, the system has already predicted the delay, pre-warmed the pricing, and queued the transaction — so the only loading spinner the user ever sees is on the final payment confirmation.

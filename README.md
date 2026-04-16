# RailPredict: High-Efficiency UK Rail Data Engine

The current version and last worked on date should be noted at the top of this file below this line:

**version = "0.1.0" -- 17/04/2026**

## Project Vision
To build a high-performance, low-latency "Shadow System" for UK Rail data that minimizes expensive API calls to Great British Railways (GBR) by using predictive logic, historical analysis, and state-machine-driven polling.

Rather than being a "dumb" mirror of the GBR API, this system acts as an **Intelligent Buffer**, providing users with "Fast-Path" data for searching and only hitting the "Slow-Path" (live GBR systems) when transactionally necessary.

---

## 1. Architectural Strategy: The "Three-Tier" Data Flow

To solve the GBR bottleneck, we separate data into three categories based on its "freshness" requirements:

### Tier A: The Static Layer (Cached)
- **Data:** Timetables, station names, base fare structures.
- **Source:** Weekly GTFS/CIF downloads from the Rail Data Marketplace.
- **Storage:** Local SQL or NoSQL database.
- **Call Cost:** Zero (Local lookup).

### Tier B: The Predictive Layer (Inferred)
- **Data:** "Likely" delay status, "Typical" platform, fare ranges.
- **Source:** Historical performance data + Real-time weather feeds + Social sentiment.
- **Logic:** Machine learning or simple statistical averages (e.g., "The 08:01 is late 85% of the time").
- **Call Cost:** Local compute only.

### Tier C: The Transactional Layer (Live)
- **Data:** Exact current location, seat availability, final ticket purchase.
- **Source:** GBR Darwin / Retail API.
- **Call Cost:** High (Latency + API Rate Limits).

---

## 2. The Logic Gate State Machine (Rust Implementation)

The core engine uses a state machine to decide the `PollingFrequency` for any given train. This prevents the system from "over-asking" for data that is unlikely to have changed.

### State Transitions
Each `TrainObject` in the system exists in one of four states:

| State         | Condition                                 | Polling Behavior                                   |
|:--------------|:------------------------------------------|:---------------------------------------------------|
| **Dormant**   | Departure > 2 hours away                  | No live calls. Use Tier A (Static) data.           |
| **Monitored** | 120 > Departure > 30 mins                 | Poll every 10 mins. Update "Probability of Delay." |
| **Active**    | 30 > Departure > 0 mins                   | High-frequency polling (30s - 60s).                |
| **Critical**  | Departure < 5 mins OR Volatility Detected | Real-time stream (Push-port) or 10s polling.       |

### Volatility Modifiers (The "Bloomberg" Logic)
The State Machine is not just time-based. It responds to external events:
- **Weather Event:** If wind > 50mph, move all trains on that route to **Active** state immediately.
- **Network Incident:** If a "Major Incident" is detected via news/social scrapers, force **Critical** state for affected corridors.

---

## 3. Reducing API Calls: Key Tactics

### A. Lazy Loading & Progressive Disclosure
Don't ask for a specific ticket price or a live platform number on the search results page.
1. **Search Page:** Show "Last Known Price" and "Scheduled Time."
2. **Selection Page:** Show "Predicted Delay" based on local history.
3. **Checkout Page:** Only now make the single, final `POST` call to GBR to lock in the ticket.

### B. Request Coalescing (Deduplication)
If 50 users are looking at the same 09:00 train to London, do not make 50 API calls.
- Implement a **Request Collapser** in Rust: If a request for `Train_ID_123` is already in flight, the other 49 users wait for that single response and share the result.

### C. Darwin "Push" vs. "Pull"
Instead of asking (Pulling), subscribe to the **Darwin Push Port (STOMP)**.
- This allows the system to receive a "Firehose" of every movement.
- You store this in a high-speed in-memory cache (Redis or `dashmap` in Rust).
- User queries are served 100% from your local cache, reducing GBR API dependency to zero for "View-only" requests.

---

## 4. Proposed Rust Tech Stack
- **Runtime:** `tokio` (Async/Await for handling thousands of concurrent users).
- **Network/API:** `reqwest` for GBR REST calls, `stomp-rs` for the Darwin Firehose.
- **Cache:** `moka` (Highly concurrent in-memory cache) or `Redis`.
- **Data Processing:** `polars` (For lightning-fast historical delay analysis).

---

## 5. Main Goal (The "North Star")
To create a rail application that feels **instant**. By the time the user has decided which train they want, the system has already "warmed up" the data, predicted the delay, and prepared the transaction, resulting in a UI that never shows a "loading spinner" until the final payment confirmation
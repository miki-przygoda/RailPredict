# TODO

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.3.0" -- 17/04/2026**

---

## How This File Works

- This is the active sprint tracker. It reflects what needs to be done **right now**.
- Larger epics live in `TODOs/` as their own files. Link to them from here when active.
- **Versioning rules (strict):**
  - Each completed TODO point → patch bump: `x.x.1 → x.x.2`
  - Each completed section/epic → minor bump: `x.1.x → x.2.0`
  - On any version bump, update the version string in: `CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml` — all five, simultaneously.
  - Note the completion in `CHANGELOG.md` before moving on.
- `TODOs/` files are created when an epic starts and **destroyed** when it is complete. On destruction: summarise into `CHANGELOG.md`, bump the minor version, update all five files.
- The version header format and this section header are **permanent** — do not modify them.

---

## Current Epics

Six epics broken out from `TODOs/Improvements.md` (see that file for the full item index
and suggested execution order). None are active yet — activate by picking one and working
through it.

| Epic | File | Items | Suggested order |
|---|---|---|---|
| Production Hardening | `TODOs/ProductionHardening.md` | 1.1, 1.2, 1.4, 1.5, 3.1, 3.2, 3.3 | 1st (with CI/DevEx in parallel) |
| CI & Developer Experience | `TODOs/CI_DevEx.md` | 1.3\*, 4.1, 4.2, 4.3, 4.4 | 1st (parallel with above) |
| Technical Debt | `TODOs/TechnicalDebt.md` | 7.1, 7.3, 9.1–9.5, 4.5† | 2nd |
| Tier A Data Layer | `TODOs/TierADataLayer.md` | 2.3r, 2.4, 2.5, 6.1, 6.2 | 3rd |
| Tier C Wiring | `TODOs/TierCWiring.md` | 2.1, 2.2, 2.6, 8.1 | 4th (largest epic) |
| Product Features | `TODOs/ProductFeatures.md` | 5.1, 5.3, 5.4, 5.5, 8.2 | 5th |

_\* Item 1.3 (.sqlx/ snapshot) requires a user action — see CI_DevEx.md for instructions._
_† Item 4.5 (dead_code removal) depends on Tier C Wiring being complete._

---

## Strategic Notes (Carry-Forward)

These are cross-cutting design decisions to keep in mind across all epics:

- **The Identifier Problem:** Use `TrainID` everywhere. Never pass raw strings for train identifiers across module boundaries.
- **The Waiter Pattern:** The coalescer in Epic 3 is the most impactful single piece of work for latency. Prioritise it.
- **Backpressure:** GBR will revoke API keys for aggressive polling. The rate limiter and circuit breaker are not optional.
- **The Ingestion Filter:** Apply the region/route filter as step one of ingestion. CPU cost compounds fast on an unfiltered firehose.

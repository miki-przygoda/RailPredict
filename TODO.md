# TODO

The current version and last worked on date should be noted at the top of this file below this line:

**version = "1.4.0" -- 18/04/2026**

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

## User Action Required

**Generate the `.sqlx/` offline snapshot** before any Docker build or CI run:

```bash
docker compose up -d db
cd RailPredict
DATABASE_URL=postgresql://railpredict:railpredict@localhost:5432/railpredict \
  cargo sqlx prepare
git add .sqlx/
git commit -m "chore: add sqlx offline snapshot"
```

The CI sqlx-check step will fail until this is done. See `TODOs/CI_DevEx.md` for full context.

---

## Remaining Epics

Epics are ordered by suggested execution sequence. Completed epics are destroyed and recorded in `CHANGELOG.md`.

| Epic | File | Items | Order |
|---|---|---|---|
| Tier A Data Layer | `TODOs/TierADataLayer.md` | 2.3r, 2.4, 2.5, 6.1, 6.2 | Next |
| Tier C Wiring | `TODOs/TierCWiring.md` | 2.1, 2.2, 2.6, 8.1 | 2nd (largest) |
| Product Features | `TODOs/ProductFeatures.md` | 5.1, 5.3, 5.4, 5.5, 8.2 | 3rd |
| Dead Code Cleanup | (inline) | 4.5 — remove `#[allow(dead_code)]` | After TierCWiring |

### Completed

| Epic | Version | Notes |
|---|---|---|
| Production Hardening | v1.2.0 | TLS, reconnect, rate limit, CORS, CRS validation, health probe, SECURITY.md |
| CI & Developer Experience | v1.3.0 | GitHub Actions, cargo-deny, DB integration tests, README fix |
| Technical Debt | v1.4.0 | 9.1–9.5, 7.1, 7.3 done; 4.5 deferred pending TierCWiring |

---

## Strategic Notes (Carry-Forward)

These are cross-cutting design decisions to keep in mind across all epics:

- **The Identifier Problem:** Use `TrainID` everywhere. Never pass raw strings for train identifiers across module boundaries.
- **The Waiter Pattern:** The coalescer in Epic 3 is the most impactful single piece of work for latency. Prioritise it.
- **Backpressure:** GBR will revoke API keys for aggressive polling. The rate limiter and circuit breaker are not optional.
- **The Ingestion Filter:** Apply the region/route filter as step one of ingestion. CPU cost compounds fast on an unfiltered firehose.

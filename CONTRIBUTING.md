# Contributing to RailPredict

---

## Getting Started

**Prerequisites:** Rust stable, PostgreSQL 15+, Python 3.11+ (for ML scripts).

```bash
git clone https://github.com/miki-przygoda/RailPredict.git
cd RailPredict/RailPredict
cp .env.example .env
# Fill in DATABASE_URL at minimum. Darwin and GBR keys are optional — see .env.example.
```

Start the database and run migrations:

```bash
docker compose up -d db        # or bring your own Postgres
cargo run --release             # migrations run automatically at startup
```

The server starts on `http://localhost:3000`. You don't need Darwin or GBR credentials to run the UI — the departure board and search work from the static timetable data alone.

**Seed data (optional, for a richer local experience):**

```bash
cd ..   # repo root
python3 scripts/seed_stations.py    # populate stations from OpenStreetMap
python3 scripts/seed_history.py     # generate synthetic delay history
```

---

## What to Read First

Start with these three files in order — they give you the full picture before touching any code:

1. **`CLAUDE.md`** — architecture reference, module map, key patterns, and the versioning protocol. This is the primary onboarding document.
2. **`docs/improvements.md`** — all architectural decisions made across the project's development history. Treat these as constraints: if you're touching a module that has a prior decision logged here, read it before writing anything.
3. **`TODO.md`** — what's currently in scope and what the four remaining epics are.

For the ML pipeline specifically, also read `docs/model-performance.md` and `docs/model-improvement-plan.md` before touching `scripts/compare_models.py`.

---

## Code Conventions

**Rust:**
- `thiserror` for domain errors, `anyhow` for application-level propagation. No `.unwrap()` in production paths — only in tests.
- All external calls go through `src/networking/`. Nothing in `src/api/` or `src/frontend/` makes outbound HTTP directly.
- `TrainId` everywhere — never pass a raw string for a train identifier across module boundaries.
- Prefer `mpsc`/`oneshot` over `Arc<Mutex<T>>`. Use `Arc<RwLock<T>>` only for read-heavy shared state.
- SQL queries are compile-time checked via `sqlx`. Run `cargo sqlx prepare` after adding or modifying queries; commit the resulting `.sqlx/` snapshot.

**Python (scripts):**
- All scripts read `DATABASE_URL` from `.env` via `python-dotenv`. No hardcoded connection strings.
- Scripts are standalone — no shared library. Each one has a module-level docstring explaining when to use it.

**Frontend:**
- `maud` for server-side HTML, `htmx` for partial updates, minimal vanilla JS only.
- No build step, no bundler, no npm. `static/style.css` is the single design system file.

---

## Versioning Protocol

This is strict — follow it on every change:

| Change type             | Version bump      |
|-------------------------|-------------------|
| Completed TODO point    | patch: `x.x.1 → x.x.2` |
| Completed epic/section  | minor: `x.1.x → x.2.0` |

When bumping, update **all five files simultaneously:**
`CLAUDE.md`, `README.md`, `TODO.md`, `CHANGELOG.md`, `RailPredict/Cargo.toml`

Then add a CHANGELOG entry before moving on.

---

## Pull Requests

- Keep PRs focused — one epic or one fix per PR.
- Run `cargo clippy -- -D warnings` and `cargo test` before opening.
- CI runs `cargo deny check` (licence + advisory scan), clippy, and the full test suite including DB integration tests. All must pass.
- The `.sqlx/` offline snapshot must be up to date if you changed any SQL queries.

---

## Using Claude Code (AI-Assisted Development)

This project was largely built with [Claude Code](https://claude.ai/code) — Anthropic's CLI for AI-assisted development. `CLAUDE.md` is the session seed that gives Claude full context of the architecture, patterns, and constraints before it writes any code.

**If you use Claude Code on this repo:**

1. Claude Code automatically reads `CLAUDE.md` at session start. You don't need to paste architecture context manually.
2. Run from the repo root so Claude can navigate the full directory tree.
3. The session warm-up order in `CLAUDE.md` ("Self-Check Notes" section) is written for Claude — it tells the model what to read first and in what order.
4. For significant new features, consider entering Plan Mode (`/plan`) before implementation. Claude will propose a design for your review before writing code.
5. Claude Code works well for: adding new API endpoints (follow the pattern in `src/api/handlers.rs`), extending the prediction engine, adding new frontend pages (follow `src/frontend/detail.rs`), and writing new migrations.

**If you don't use Claude Code:** `CLAUDE.md` is also a useful human architecture reference — the directory map and key patterns sections are worth reading regardless.

---

## Project Structure at a Glance

```
RailPredict/src/
├── api/          REST handlers, SSE, response types
├── cache/        DashMap train registry + station autocomplete index
├── db/           sqlx queries — history, timetables, predictions
├── frontend/     maud server-side HTML pages
├── ingestion/    Darwin STOMP client, XML parser, GTFS loader
├── networking/   Coalescer, circuit breaker, rate limiter, GBR client
├── prediction/   Tier B engine + ONNX inference
├── state_machine/ Urgency states + poll scheduler
├── types/        Shared domain types
└── weather/      Open-Meteo polling + volatility scoring

scripts/          Python ML training, data export, and DB seeding
migrations/       sqlx Postgres migrations (versioned, checksum-locked)
docs/             Architecture decisions, ML performance notes
```

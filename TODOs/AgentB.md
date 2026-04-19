# Agent B — Journey Search (A→B)

_Item: 5.1_

**File ownership — Agent B only:**
- `RailPredict/src/api/handlers.rs`
- `RailPredict/src/api/mod.rs`
- `RailPredict/src/frontend/search.rs`

**No overlap with Agent A or Agent C.** Do not touch any other files.

---

## 5.1 — Journey search (A→B, not just departures from A)

The current UI shows all departures from a single origin. Journey search filters the
timetable to services that call both an origin AND a destination station in order (the
origin's `call_order` must be less than the destination's `call_order` for the same service).

### 1. Backend — `src/api/handlers.rs`

Add a `JourneyQuery` struct and a `journey_handler` function:

```rust
#[derive(Deserialize)]
pub struct JourneyQuery {
    pub from: String,
    pub to: String,
    pub date: Option<String>, // YYYY-MM-DD; defaults to today
}
```

```rust
pub async fn journey_handler(
    Query(params): Query<JourneyQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // Validate both CRS codes.
    let from = validate_crs(&params.from)?;
    let to   = validate_crs(&params.to)?;
    // ...
}
```

The DB query to execute (write as a `sqlx::query_as!` in `handlers.rs` directly, or in
`src/db/static_data.rs` as `journeys_between` — your choice, but keep it consistent with
the existing pattern in `static_data.rs`):

```sql
SELECT
    tc_from.trip_id,
    tc_from.uid,
    tc_from.scheduled_departure AS origin_departure,
    tc_to.scheduled_arrival     AS destination_arrival,
    tc_from.platform            AS origin_platform
FROM timetable_calls tc_from
JOIN timetable_calls tc_to
    ON tc_to.trip_id      = tc_from.trip_id
   AND tc_to.location_crs = $2               -- destination CRS
   AND tc_to.call_order   > tc_from.call_order  -- destination must be after origin
WHERE tc_from.location_crs = $1              -- origin CRS
  AND tc_from.operating_date = $3            -- date
ORDER BY tc_from.scheduled_departure;
```

Map each result row into a `DepartureBoardEntry` (reuse the existing DTO — populate
`destination_name` with the `to` CRS initially; the caller can resolve the name separately
if needed). Return `Json(Vec<DepartureBoardEntry>)` on success, `StatusCode::BAD_REQUEST`
on CRS validation failure.

**Check `timetable_calls` schema** before writing the query — use
`migrations/20240417120002_create_timetable_calls.sql` as the reference for column names.
In particular verify: `trip_id`, `uid`, `call_order`, `location_crs`, `operating_date`,
`scheduled_departure`, `scheduled_arrival`, `platform` exist.

### 2. Route — `src/api/mod.rs`

Add to `build_api_router`:
```rust
.route("/journeys", get(handlers::journey_handler))
```

Add to `infra_router` (no rate limiting needed — it's a read-only timetable query, same as
`/stations/{crs}/departures`):
Actually, add it to `build_api_router` alongside the existing departures route so it gets
the governor rate limiter. That's the right place.

Also add the UI fragment route:
```rust
.route("/ui/journeys", get(search::journeys_fragment))
```

### 3. Frontend — `src/frontend/search.rs`

The current departure search form has a single "From" station input. Extend it to optionally
accept a "To" input.

**Design:** when the "To" field is empty, the form behaves exactly as today (departures from
origin). When "To" is filled, `hx-get` points at `/ui/journeys` instead of
`/ui/stations/departures`. Use a small inline `<script>` or `hx-vals` to switch the target
dynamically, or simply add a second submit path.

The simplest correct approach: two separate `<form>` blocks, one for departures and one for
journeys. Show a tab toggle (two `<button>` elements styled as tabs) that reveals one form
at a time using a hidden CSS class. No JS frameworks needed — htmx handles the swaps.

```html
<!-- Tab buttons -->
<div class="search-tabs">
  <button class="tab-btn active" onclick="showTab('departures')">Departures</button>
  <button class="tab-btn" onclick="showTab('journeys')">Journey</button>
</div>

<!-- Departures form (existing, unchanged) -->
<form id="tab-departures" ...>...</form>

<!-- Journey form (new) -->
<form id="tab-journeys" class="hidden"
      hx-get="/ui/journeys"
      hx-target="#results"
      hx-trigger="submit">
  ... two autocomplete inputs for from/to ...
</form>
```

The tab JS (inline `<script>` in the maud template) only needs:
```js
function showTab(name) {
    document.querySelectorAll('.tab-pane').forEach(el => el.classList.add('hidden'));
    document.getElementById('tab-' + name).classList.remove('hidden');
    document.querySelectorAll('.tab-btn').forEach(el => el.classList.remove('active'));
    event.target.classList.add('active');
}
```

Add `journeys_fragment` handler in `search.rs`:
- Accepts `JourneyQuery` (same struct or a local copy for the fragment).
- Calls `handlers::journey_handler` logic (or duplicates the DB call — either is fine).
- Renders the same `departure_card` list used by `departures_fragment`.
- If `from == to`, return an error message fragment instead of results.

### CSS additions (`RailPredict/static/style.css`)

```css
.search-tabs { display: flex; gap: 0.5rem; margin-bottom: 1rem; }
.tab-btn { padding: 0.4rem 1rem; border: 1px solid #ccc; border-radius: 4px; cursor: pointer; background: #f5f5f5; }
.tab-btn.active { background: #1a73e8; color: white; border-color: #1a73e8; }
.hidden { display: none !important; }
```

---

## Validation

After implementing:
1. `cargo build` passes with zero warnings.
2. `cargo clippy -- -D warnings` passes.
3. `cargo test --lib` passes (no new tests required, but a unit test for CRS validation on
   the journey handler is welcome).
4. Manually verify: with `timetable_calls` populated, `GET /journeys?from=LDS&to=MAN` returns
   a non-empty list of services that call both Leeds and Manchester in order.

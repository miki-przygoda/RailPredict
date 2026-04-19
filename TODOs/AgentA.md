# Agent A — Fare Display + DB Partitioning

_Items: 5.3, 8.2_

**File ownership — Agent A only:**
- `RailPredict/src/frontend/detail.rs`
- `migrations/20240419_partition_delay_history.sql` (new file)

**No overlap with Agent B or Agent C.** Do not touch any other files.

---

## 5.3 — Fare display on train detail page

`db::static_data::cheapest_fare` exists and is callable from handlers but is never shown
in the UI. The train detail page already reads `origin_crs` from the registry. It needs to
also capture `destination_crs` and pass both to `cheapest_fare`.

### What to do in `src/frontend/detail.rs`

1. **Extend the snapshot tuple** to also capture `destination_crs: Option<String>` from the
   registry alongside the existing fields (origin_crs, scheduled_departure, etc.). Look at
   `TrainStatus` — `destination_crs` is a field stored as `Option<String>`.

2. **Call `cheapest_fare`** from the handler, after unpacking the snapshot:
   ```rust
   let fare_pence: Option<i32> = if let (Some(o), Some(d)) = (&origin, &dest) {
       crate::db::static_data::cheapest_fare(&state.db, o, d).await.ok().flatten()
   } else {
       None
   };
   ```

3. **Add a `pence_to_pounds` helper** in the same file (private, not exported):
   ```rust
   fn pence_to_pounds(pence: i32) -> String {
       format!("£{:.2}", pence as f64 / 100.0)
   }
   ```

4. **Render the fare chip** in the `base()` call, directly after the delay_badge/platform_chip
   row, only when `fare_pence.is_some()`:
   ```rust
   @if let Some(pence) = fare_pence {
       span .fare-chip { "From " (pence_to_pounds(pence)) }
   }
   ```

5. **Add `.fare-chip` CSS** to `RailPredict/static/style.css`:
   ```css
   .fare-chip {
       background: #e8f5e9;
       color: #2e7d32;
       border: 1px solid #a5d6a7;
       border-radius: 4px;
       padding: 2px 8px;
       font-size: 0.85rem;
       font-weight: 600;
   }
   ```

   Wait — `static/style.css` is embedded at compile time via `rust-embed`. You CAN edit it.
   Add the `.fare-chip` rule there.

### Notes
- `cheapest_fare` returns `anyhow::Result<Option<i32>>` — use `.ok().flatten()`.
- `detail.rs` already has `State(state): State<AppState>` so `state.db` is directly available.
- The fare is optional (many origin/destination pairs have no fare record yet). Render nothing
  when `None` — do not show a placeholder.

---

## 8.2 — `delay_history` range partitioning

This is a SQL migration only. No Rust code changes.

### What to do

Create `migrations/20240419_partition_delay_history.sql` with the following approach:

```sql
-- Convert delay_history to a range-partitioned table on recorded_at (quarterly).
--
-- NOTE: Postgres cannot partition an existing table in-place. This migration:
-- 1. Renames the existing table to delay_history_legacy.
-- 2. Creates a new partitioned table delay_history with the same schema.
-- 3. Copies data from the legacy table into the new partitioned table.
-- 4. Drops the legacy table.
--
-- For existing installs with large data volumes, run steps 1-3 manually
-- during a maintenance window, then run step 4 after verifying row counts.

BEGIN;

ALTER TABLE delay_history RENAME TO delay_history_legacy;

CREATE TABLE delay_history (
    id          BIGSERIAL,
    uid         TEXT        NOT NULL,
    weekday     SMALLINT    NOT NULL,
    origin_crs  TEXT        NOT NULL,
    delay_mins  INTEGER     NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT delay_history_pkey PRIMARY KEY (id, recorded_at)
) PARTITION BY RANGE (recorded_at);

-- Create partitions for: previous quarter, current quarter, next two quarters.
-- Adjust the date boundaries to match the current year when adding new partitions.
CREATE TABLE delay_history_2024_q1 PARTITION OF delay_history
    FOR VALUES FROM ('2024-01-01') TO ('2024-04-01');
CREATE TABLE delay_history_2024_q2 PARTITION OF delay_history
    FOR VALUES FROM ('2024-04-01') TO ('2024-07-01');
CREATE TABLE delay_history_2024_q3 PARTITION OF delay_history
    FOR VALUES FROM ('2024-07-01') TO ('2024-10-01');
CREATE TABLE delay_history_2024_q4 PARTITION OF delay_history
    FOR VALUES FROM ('2024-10-01') TO ('2025-01-01');
CREATE TABLE delay_history_2025_q1 PARTITION OF delay_history
    FOR VALUES FROM ('2025-01-01') TO ('2025-04-01');
CREATE TABLE delay_history_2025_q2 PARTITION OF delay_history
    FOR VALUES FROM ('2025-04-01') TO ('2025-07-01');
CREATE TABLE delay_history_2025_q3 PARTITION OF delay_history
    FOR VALUES FROM ('2025-07-01') TO ('2025-10-01');
CREATE TABLE delay_history_2025_q4 PARTITION OF delay_history
    FOR VALUES FROM ('2025-10-01') TO ('2026-01-01');
CREATE TABLE delay_history_2026_q1 PARTITION OF delay_history
    FOR VALUES FROM ('2026-01-01') TO ('2026-04-01');
CREATE TABLE delay_history_2026_q2 PARTITION OF delay_history
    FOR VALUES FROM ('2026-04-01') TO ('2026-07-01');

-- Recreate the index on the partitioned table (created per-partition automatically in PG14+).
CREATE INDEX ON delay_history (uid, weekday, origin_crs, recorded_at);

-- Copy existing data from legacy table.
INSERT INTO delay_history (uid, weekday, origin_crs, delay_mins, recorded_at)
SELECT uid, weekday, origin_crs, delay_mins, recorded_at
FROM delay_history_legacy;

-- Drop legacy table after verifying row counts:
--   SELECT COUNT(*) FROM delay_history_legacy;
--   SELECT COUNT(*) FROM delay_history;
-- Uncomment the line below once counts match.
-- DROP TABLE delay_history_legacy;

COMMIT;
```

### Notes
- The existing unique index on `delay_history` uses `(uid, weekday, origin_crs, recorded_at)`.
  The partitioned version recreates this as a non-unique index — the PK already enforces
  uniqueness per partition and Postgres requires the partition key (`recorded_at`) to be
  included in any unique constraint on a partitioned table.
- `flush_history` in `db/history.rs` uses `ON CONFLICT DO NOTHING` — this continues to work
  correctly on a partitioned table.
- `load_history` uses a window function with no partition-key filter — add a `recorded_at >
  NOW() - INTERVAL '91 days'` predicate to ensure only recent partitions are scanned.
- The `DROP TABLE delay_history_legacy` is intentionally commented out — leave it commented.
  Operators should verify row counts manually before dropping the legacy table.
- Add a comment at the top of the migration: `-- TimescaleDB is the recommended next step
  if full national-network scale is targeted.`

---

## Validation

After implementing both items:
1. `cargo build` must pass with zero warnings.
2. `cargo clippy -- -D warnings` must pass.
3. `cargo test --lib` must pass (no new tests are required for 8.2; a snapshot test for
   `pence_to_pounds` is welcome but not mandatory).

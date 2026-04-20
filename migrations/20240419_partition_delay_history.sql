-- TimescaleDB is the recommended next step if full national-network scale is targeted.
--
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

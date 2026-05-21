-- TimescaleDB is the recommended next step if full national-network scale is targeted.
--
-- Convert delay_history to a range-partitioned table on recorded_at (quarterly).
--
-- NOTE: Postgres cannot partition an existing table in-place. This migration:
-- 1. Renames the existing table to delay_history_legacy.
-- 2. Renames the primary key constraint/index that moves with the table rename
--    (constraint names are schema-scoped, so the old delay_history_pkey would
--    block the new CREATE TABLE if not renamed first).
-- 3. Creates a new partitioned table delay_history with the same schema.
-- 4. Copies data from the legacy table into the new partitioned table.
-- 5. Drops the legacy table.
--
-- DO NOT add explicit BEGIN/COMMIT here — sqlx wraps every migration in its own
-- transaction. An explicit COMMIT would commit sqlx's transaction before it can
-- record the migration in _sqlx_migrations, causing an infinite retry loop.

ALTER TABLE delay_history RENAME TO delay_history_legacy;

-- The primary key constraint and its backing index keep their original name after
-- the table rename. Rename them now so the new CREATE TABLE can reuse the name.
ALTER INDEX delay_history_pkey RENAME TO delay_history_legacy_pkey;

CREATE TABLE delay_history (
    id             BIGSERIAL,
    uid            TEXT        NOT NULL,
    weekday        SMALLINT    NOT NULL,
    origin_crs     TEXT        NOT NULL,
    delay_mins     INTEGER     NOT NULL,
    recorded_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- departure_hour was added by 20240417120005; must be present in the new
    -- partitioned table so that the window-function query in db/history.rs works.
    departure_hour SMALLINT    NOT NULL DEFAULT 0,
    CONSTRAINT delay_history_pkey        PRIMARY KEY (id, recorded_at),
    CONSTRAINT chk_departure_hour        CHECK (departure_hour BETWEEN 0 AND 23)
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

-- Recreate the observation uniqueness constraint on the partitioned table.
-- Must be UNIQUE (not just an index) so that flush_history's ON CONFLICT clause works.
-- Must include the partition key (recorded_at) — PG requires all unique constraints on
-- partitioned tables to include every partition key column.
-- PG14+ propagates this index automatically to each new partition.
CREATE UNIQUE INDEX ON delay_history (uid, weekday, origin_crs, departure_hour, recorded_at);

-- Copy existing data from legacy table (including departure_hour added by 20240417120005).
INSERT INTO delay_history (uid, weekday, origin_crs, delay_mins, recorded_at, departure_hour)
SELECT uid, weekday, origin_crs, delay_mins, recorded_at, departure_hour
FROM delay_history_legacy;

-- Drop legacy table now that data has been copied.
-- The legacy table held no data in most installs; verify counts if unsure:
--   SELECT COUNT(*) FROM delay_history_legacy;
--   SELECT COUNT(*) FROM delay_history;
DROP TABLE delay_history_legacy;

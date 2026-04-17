-- Phase 3 (AdvancedAnalytics): add departure_hour to delay_history.
--
-- Splits the (uid, weekday, origin_crs) pattern into per-hour buckets so that
-- rush-hour delays (e.g. 08:00 service) no longer corrupt off-peak predictions
-- (e.g. 14:00 service with the same UID). Each hour of the day is tracked
-- independently in the in-memory HistoricalStore.
--
-- DEFAULT 0 ensures existing rows (which predate this migration) are not rejected.
-- They will be bucketed into the "midnight" hour, which is safe: the prediction
-- engine will simply treat them as hour-0 observations until new data accumulates
-- for the correct hour buckets.
--
-- The unique index is rebuilt to include departure_hour, preventing duplicate flush
-- writes for a given (uid, weekday, origin_crs, hour, timestamp) tuple.

ALTER TABLE delay_history
    ADD COLUMN IF NOT EXISTS departure_hour SMALLINT NOT NULL DEFAULT 0;

ALTER TABLE delay_history
    ADD CONSTRAINT chk_departure_hour CHECK (departure_hour BETWEEN 0 AND 23);

-- Drop and recreate the unique index to include departure_hour.
DROP INDEX IF EXISTS dh_unique_observation_idx;

CREATE UNIQUE INDEX IF NOT EXISTS dh_unique_observation_idx
    ON delay_history(uid, weekday, origin_crs, departure_hour, recorded_at);

-- Recreate the primary pattern lookup index to include departure_hour.
-- This makes the per-hour load query (window function PARTITION BY) use the index.
DROP INDEX IF EXISTS dh_pattern_idx;

CREATE INDEX IF NOT EXISTS dh_pattern_idx
    ON delay_history(uid, weekday, origin_crs, departure_hour);

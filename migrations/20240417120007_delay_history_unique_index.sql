-- Add the unique observation index that flush_history's ON CONFLICT clause requires.
--
-- Migration 006 converted delay_history to a partitioned table and recreated only a
-- plain index. ON CONFLICT (uid, weekday, origin_crs, departure_hour, recorded_at)
-- requires a unique constraint that includes the partition key (recorded_at).
--
-- PG14+ propagates this index automatically to each new partition.
CREATE UNIQUE INDEX IF NOT EXISTS dh_unique_obs_idx
    ON delay_history (uid, weekday, origin_crs, departure_hour, recorded_at);

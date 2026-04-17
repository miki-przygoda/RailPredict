-- Tier B: persistent backing store for the in-memory HistoricalStore.
-- The in-memory DashMap is the hot path; this table is the source of truth
-- on startup and the flush target every 60 seconds.
--
-- No FK to services(uid): a service may be retired from the timetable while
-- its delay history remains historically valid for the prediction engine.
--
-- weekday encoding: 0 = Monday, 6 = Sunday (matches chrono::Weekday::num_days_from_monday).

CREATE TABLE IF NOT EXISTS delay_history (
    id          BIGSERIAL    PRIMARY KEY,
    uid         CHAR(6)      NOT NULL,
    weekday     SMALLINT     NOT NULL CHECK (weekday BETWEEN 0 AND 6),
    origin_crs  CHAR(3)      NOT NULL,
    delay_mins  INTEGER      NOT NULL,
    recorded_at TIMESTAMPTZ  NOT NULL DEFAULT now()
);

-- Primary access pattern: load all records for a (uid, weekday, origin_crs) pattern.
CREATE INDEX IF NOT EXISTS dh_pattern_idx     ON delay_history(uid, weekday, origin_crs);
-- Used by the load query to take only the most recent MAX_SAMPLES rows per pattern.
CREATE INDEX IF NOT EXISTS dh_recorded_at_idx ON delay_history(recorded_at DESC);

-- Composite unique index prevents duplicate flush writes.
-- (uid, weekday, origin_crs, recorded_at) is effectively unique in practice
-- because recorded_at carries microsecond precision.
CREATE UNIQUE INDEX IF NOT EXISTS dh_unique_observation_idx
    ON delay_history(uid, weekday, origin_crs, recorded_at);

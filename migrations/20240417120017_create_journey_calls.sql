-- Full-Journey Capture (Phase 1): per-stop detail for each journey.
--
-- One row per calling point of a finalised service, written alongside the
-- journeys header (see ingestion/mod.rs / db/journeys.rs). Enables trajectory
-- (recovering vs deteriorating), per-segment delay attribution (where delay is
-- incurred), and dwell-anomaly analysis — none of which the single-number
-- delay_history could express.
--
-- seq orders the calls along the journey (0 = origin). dwell_secs = actual_dep -
-- actual_arr when both are known. UNIQUE(rid, seq) makes re-inserts idempotent.

CREATE TABLE IF NOT EXISTS journey_calls (
    id             BIGSERIAL    PRIMARY KEY,
    rid            CHAR(15)     NOT NULL,
    seq            SMALLINT     NOT NULL,
    tpl            VARCHAR(8)   NOT NULL,

    sched_arr      TIMESTAMPTZ,
    actual_arr     TIMESTAMPTZ,
    arr_delay_mins INTEGER,

    sched_dep      TIMESTAMPTZ,
    actual_dep     TIMESTAMPTZ,
    dep_delay_mins INTEGER,

    platform       VARCHAR(4),
    plat_confirmed BOOLEAN,
    is_cancelled   BOOLEAN      NOT NULL DEFAULT FALSE,
    dwell_secs     INTEGER
);

-- Idempotent re-insert guard (one row per stop per service).
CREATE UNIQUE INDEX IF NOT EXISTS journey_calls_rid_seq_idx ON journey_calls (rid, seq);
-- Per-service lookup (load the whole journey).
CREATE INDEX IF NOT EXISTS journey_calls_rid_idx ON journey_calls (rid);
-- Cross-service "where is delay incurred at this location" queries.
CREATE INDEX IF NOT EXISTS journey_calls_tpl_idx ON journey_calls (tpl);

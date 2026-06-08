-- Full-Journey Capture (Phase 1): per-service "fat record" header.
--
-- One row per finalised RID, written best-effort when a train deactivates (see
-- ingestion/mod.rs). Distinct from delay_history (pattern-aggregated, feeds the
-- live model) and prediction_outcomes (predicted-vs-actual ledger): this captures
-- the WHOLE journey we previously distilled to a single origin-delay integer —
-- arrival delay, reason code, operator, recovery, platform — for regression /
-- relationship analysis and richer dashboards.
--
-- Additive: the live predictor still reads delay_history unchanged. No FK to
-- services(uid) — a service may be retired while its history stays valid.
-- weekday: 0 = Monday .. 6 = Sunday (matches delay_history). Per-stop detail
-- lives in journey_calls. reason_class: 0 = unknown, 1 = structural, 2 = exogenous.

CREATE TABLE IF NOT EXISTS journeys (
    rid                 CHAR(15)    PRIMARY KEY,
    uid                 VARCHAR(8)  NOT NULL,
    ssd                 DATE        NOT NULL,
    weekday             SMALLINT    NOT NULL CHECK (weekday BETWEEN 0 AND 6),
    departure_hour      SMALLINT    NOT NULL CHECK (departure_hour BETWEEN 0 AND 23),

    -- Service identity / classification (from the `schedule` message).
    toc                 CHAR(2),
    train_category      VARCHAR(8),

    -- Endpoints + headline timings.
    origin_tpl          VARCHAR(8)  NOT NULL,
    destination_tpl     VARCHAR(8),
    scheduled_departure TIMESTAMPTZ NOT NULL,
    actual_departure    TIMESTAMPTZ,
    origin_delay_mins   INTEGER,
    arrival_delay_mins  INTEGER,                 -- delay at the destination (NEW)

    -- "Why": late/cancel reason codes + derived class.
    late_reason_code    SMALLINT,
    cancel_reason_code  SMALLINT,
    reason_tiploc       VARCHAR(8),
    reason_class        SMALLINT    NOT NULL DEFAULT 0,

    -- Outcome flags.
    was_cancelled       BOOLEAN     NOT NULL DEFAULT FALSE,
    partial_cancel      BOOLEAN     NOT NULL DEFAULT FALSE,

    -- Cheap rollups over the calls, computed at finalisation.
    n_calls             SMALLINT,
    max_delay_mins      INTEGER,
    min_delay_mins      INTEGER,
    recovered_mins      INTEGER,                 -- max_delay_mins - arrival_delay_mins

    -- Platform + environment.
    origin_platform     VARCHAR(4),
    platform_confirmed  BOOLEAN,
    wind_mph            REAL,

    finalised_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS journeys_uid_idx        ON journeys (uid);
CREATE INDEX IF NOT EXISTS journeys_toc_idx        ON journeys (toc);
CREATE INDEX IF NOT EXISTS journeys_pattern_idx    ON journeys (weekday, departure_hour);
CREATE INDEX IF NOT EXISTS journeys_finalised_idx  ON journeys (finalised_at DESC);

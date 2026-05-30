-- Synthetic training data: on-time and average operating days generated from
-- real service patterns.  Stores pre-computed features alongside the base
-- delay signal so training can use them directly without re-deriving from the
-- (disrupted) real rolling history.
--
-- day_type:   'good'    ~95% on-time, low rolling stats
--             'average' ~70% on-time, moderate rolling stats
-- synth_week: 1–3 (three synthetic weeks covering Mon–Fri each)
-- generation: e.g. 'v7-2026-05-30' — retrain tag for provenance tracking

CREATE TABLE delay_history_synthetic (
    id                      BIGSERIAL PRIMARY KEY,
    uid                     TEXT        NOT NULL,
    weekday                 SMALLINT    NOT NULL,
    origin_crs              TEXT        NOT NULL,
    departure_hour          SMALLINT    NOT NULL,

    -- target
    delay_mins              INTEGER     NOT NULL,

    -- pre-computed rolling features (self-consistent with day_type, not
    -- derived from the disrupted real-data window)
    rolling_mean_7d         REAL        NOT NULL,
    rolling_std_7d          REAL        NOT NULL,
    rolling_ontime_7d       REAL        NOT NULL,
    rolling_mean_14d        REAL        NOT NULL,
    rolling_std_14d         REAL        NOT NULL,

    -- pre-computed live features
    current_delay_mins      REAL        NOT NULL,
    preceding_delay_mins    REAL        NOT NULL,
    mins_until_departure    REAL        NOT NULL,
    wind_mph                REAL        NOT NULL,
    volatility_score        REAL        NOT NULL DEFAULT 0,
    station_congestion_30m  REAL        NOT NULL,
    operator_cascade_delay  REAL        NOT NULL,
    predecessor_train_delay REAL        NOT NULL,

    -- synthetic metadata
    day_type                TEXT        NOT NULL CHECK (day_type IN ('good', 'average')),
    synth_week              SMALLINT    NOT NULL CHECK (synth_week BETWEEN 1 AND 3),
    generation              TEXT        NOT NULL,

    -- fake timestamp: week 1 = 2026-04-07 Mon, week 2 = 2026-04-14, week 3 = 2026-04-21
    -- anchored in the past so they sort before real May data
    synthetic_date          DATE        NOT NULL
);

CREATE INDEX idx_synth_uid_weekday
    ON delay_history_synthetic (uid, weekday, origin_crs, departure_hour);
CREATE INDEX idx_synth_generation
    ON delay_history_synthetic (generation);
CREATE INDEX idx_synth_day_type
    ON delay_history_synthetic (day_type, synth_week);

-- Persisted cancellation observations — one row per cancelled service, captured when
-- a cancelled train deactivates. Parallel to delay_history (which holds delay
-- observations of services that ran): together they give a real historical
-- "% cancelled" rather than an estimate from live registry state.
--
-- No FK to services(uid): a service may be retired from the timetable while its
-- cancellation history remains historically valid. weekday: 0 = Monday .. 6 = Sunday
-- (matches delay_history). Best-effort, forward-only — only accrues from deploy.

CREATE TABLE IF NOT EXISTS cancellations (
    id             BIGSERIAL    PRIMARY KEY,
    uid            CHAR(6)      NOT NULL,
    origin_crs     CHAR(3)      NOT NULL,
    weekday        SMALLINT     NOT NULL CHECK (weekday BETWEEN 0 AND 6),
    departure_hour SMALLINT     NOT NULL CHECK (departure_hour BETWEEN 0 AND 23),
    recorded_at    TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS cancellations_recorded_at_idx ON cancellations (recorded_at DESC);
CREATE INDEX IF NOT EXISTS cancellations_uid_idx          ON cancellations (uid);

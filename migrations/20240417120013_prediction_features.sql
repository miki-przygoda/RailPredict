-- Store the ML feature vector used at prediction time.
-- Enables replaying predictions and building a clean (features → final_delay_mins)
-- training dataset from real-world outcomes rather than the noisy delay_history stream.
ALTER TABLE prediction_outcomes ADD COLUMN IF NOT EXISTS features JSONB;

-- One row per significant prediction event per train.
-- Unlike prediction_outcomes (one row per RID), this table accumulates across the journey:
-- initial prediction, re-predictions on state transitions, and emergency Critical promotions.
-- Primary use: per-train prediction timeline and lead-time accuracy analysis.
CREATE TABLE IF NOT EXISTS prediction_snapshots (
    id                   BIGSERIAL    PRIMARY KEY,
    rid                  CHAR(15)     NOT NULL,
    uid                  VARCHAR(8)   NOT NULL,
    predicted_delay_mins INTEGER      NOT NULL,
    features             JSONB,
    snapshotted_at       TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS ps_rid_idx     ON prediction_snapshots (rid);
CREATE INDEX IF NOT EXISTS ps_snap_at_idx ON prediction_snapshots (snapshotted_at DESC);

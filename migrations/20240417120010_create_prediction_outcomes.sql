-- Per-train predicted-vs-actual outcome ledger.
--
-- Distinct from `delay_history` (which is pattern-aggregated and feeds the model).
-- One row per Train Instance (RID). Inserted when the prediction engine first
-- produces a delay estimate for that RID; the final_delay_mins / finalised_at
-- columns are updated when the train deactivates (Terminal state).
--
-- The "first prediction" is the one we keep — refinements live in the registry's
-- live TrainStatus. This lets us fairly compare "what we predicted up front"
-- against "what actually happened" without late-binding bias.

CREATE TABLE IF NOT EXISTS prediction_outcomes (
    rid                              CHAR(15)    PRIMARY KEY,
    uid                              VARCHAR(8)  NOT NULL,
    origin_crs                       CHAR(3)     NOT NULL,
    destination_crs                  CHAR(3),
    scheduled_departure              TIMESTAMPTZ NOT NULL,

    -- Prediction snapshot, captured at insert time.
    predicted_delay_mins             INTEGER     NOT NULL,
    prediction_confidence            REAL,
    correlation_preceding_rid        CHAR(15),
    correlation_preceding_delay_mins INTEGER,
    predicted_at                     TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Outcome, filled in on deactivation.
    final_delay_mins                 INTEGER,
    finalised_at                     TIMESTAMPTZ
);

-- "Recent predictions" feed (dev panel).
CREATE INDEX IF NOT EXISTS po_predicted_at_idx
    ON prediction_outcomes (predicted_at DESC);

-- "Recent finalised outcomes" — partial index keeps it small.
CREATE INDEX IF NOT EXISTS po_finalised_at_idx
    ON prediction_outcomes (finalised_at DESC)
    WHERE finalised_at IS NOT NULL;

-- Per-service-uid lookups (e.g. "how has C12345 been doing this week").
CREATE INDEX IF NOT EXISTS po_uid_idx
    ON prediction_outcomes (uid);

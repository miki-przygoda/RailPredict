-- Store what the engine predicted at the moment each actual delay was recorded.
-- NULL for rows written before this migration (no prediction was captured at that time).
-- Used by the export-site command to compute predicted-vs-actual accuracy over time.

ALTER TABLE delay_history
    ADD COLUMN IF NOT EXISTS predicted_delay_mins INTEGER;

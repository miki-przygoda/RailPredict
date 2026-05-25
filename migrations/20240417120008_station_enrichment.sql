-- Station enrichment columns.
--
-- tiploc: 7-char Network Rail TIPLOC code. Populated from CORPUS/BPLAN reference
--   data when available; nullable until then. Needed to correlate Darwin `tpl`
--   attributes back to CRS for last_seen_at updates.
--
-- last_seen_at: wall-clock time of the most recent Darwin TS message for this
--   station. Updated by the poll consumer when a TIPLOC lookup succeeds.
--   NULL = never seen in the Darwin stream (or tiploc not yet mapped).

ALTER TABLE stations
    ADD COLUMN IF NOT EXISTS tiploc     TEXT,
    ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS stations_tiploc_idx
    ON stations (tiploc) WHERE tiploc IS NOT NULL;

-- Speeds up the per-station train-count subquery in the autocomplete endpoint.
CREATE INDEX IF NOT EXISTS timetable_calls_crs_date_idx
    ON timetable_calls (location_crs, operating_date);

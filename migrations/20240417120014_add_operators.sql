-- Operator (TOC) identity for per-operator analytics.
--
-- services.toc holds the GTFS agency_id derived from agency.txt/routes.txt
-- (the most reliable operator source in the NR feed). Because services.uid is
-- the PK and both delay_history and prediction_outcomes are keyed on uid,
-- a single toc column on services unlocks operator grouping on ALL historic
-- data via a query-time JOIN — no per-row backfill of the large tables.
--
-- The operators table maps that agency_id to a friendly name + brand colour
-- for the dashboard. Names come from agency.txt; colours from a curated map.

ALTER TABLE services ADD COLUMN IF NOT EXISTS toc TEXT;
CREATE INDEX IF NOT EXISTS services_toc_idx ON services(toc);

CREATE TABLE IF NOT EXISTS operators (
    toc         TEXT        PRIMARY KEY,
    name        TEXT        NOT NULL,
    brand_color CHAR(7)     NOT NULL DEFAULT '#9aa7b4',
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

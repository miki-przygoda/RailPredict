-- Tier A: UK rail stations.
-- Source: GTFS stops.txt or CIF station master file (weekly refresh).
-- CRS is the 3-letter CRS code used throughout Darwin and the GBR API.

CREATE TABLE IF NOT EXISTS stations (
    crs         CHAR(3)          PRIMARY KEY,
    name        TEXT             NOT NULL,
    -- National Location Code — 4-digit numeric, not always present in GTFS.
    nlc         CHAR(4),
    lat         DOUBLE PRECISION,
    lon         DOUBLE PRECISION,
    is_active   BOOLEAN          NOT NULL DEFAULT TRUE,
    -- Refreshed on each GTFS ingest; used to detect stale data.
    updated_at  TIMESTAMPTZ      NOT NULL DEFAULT now()
);

-- Allow fast lookups by name prefix (station search box).
CREATE INDEX IF NOT EXISTS stations_name_idx ON stations USING gin(to_tsvector('english', name));

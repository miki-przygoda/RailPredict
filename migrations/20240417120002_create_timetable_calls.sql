-- Tier A: individual station calls within a service on a given operating date.
-- One row per (uid, operating_date, location_crs) combination.
-- This is the largest table: ~500k rows/week for a full UK feed.
--
-- Partition note: add range partitioning on operating_date (quarterly) before
-- going to production scale. For local dev a single table is fine.

CREATE TABLE IF NOT EXISTS timetable_calls (
    id                  BIGSERIAL    PRIMARY KEY,
    uid                 CHAR(6)      NOT NULL REFERENCES services(uid),
    operating_date      DATE         NOT NULL,
    location_crs        CHAR(3)      NOT NULL REFERENCES stations(crs),
    -- 0 = origin call, ascending toward destination.
    call_order          SMALLINT     NOT NULL,
    scheduled_departure TIME,
    public_departure    TIME,
    -- Platform may not be in the timetable (assigned day-of by Darwin).
    platform            TEXT
);

-- Hot query: all departures from a station on a given date (departure board).
CREATE INDEX IF NOT EXISTS tc_location_date_idx ON timetable_calls(location_crs, operating_date);
-- Hot query: all calls for a service on a given date (train detail page).
CREATE INDEX IF NOT EXISTS tc_uid_date_idx      ON timetable_calls(uid, operating_date);

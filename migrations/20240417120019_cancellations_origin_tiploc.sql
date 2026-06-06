-- Cancellations are recorded with the same TIPLOC-style origin code as delay_history /
-- journeys (e.g. HBOLTN), not a 3-letter CRS. The CHAR(3) column would overflow on insert
-- (and is why no cancellation ever persisted). Widen it to match.
ALTER TABLE cancellations ALTER COLUMN origin_crs TYPE VARCHAR(8);

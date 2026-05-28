-- Darwin stores TIPLOCs (up to 7 chars, e.g. "WATRLMN") in origin_crs / destination_crs,
-- not 3-letter CRS codes. Widen both columns so inserts don't fail on TIPLOC values.

ALTER TABLE prediction_outcomes
    ALTER COLUMN origin_crs     TYPE VARCHAR(8),
    ALTER COLUMN destination_crs TYPE VARCHAR(8);

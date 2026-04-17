-- Tier A: base fare data.
-- Prices are stored in pence (integer) to avoid floating-point rounding errors.
-- Validity windows allow multiple fares to coexist; the API layer selects the
-- one valid today via valid_from <= current_date AND (valid_to IS NULL OR valid_to >= current_date).

CREATE TABLE IF NOT EXISTS fares (
    origin_crs      CHAR(3)  NOT NULL REFERENCES stations(crs),
    destination_crs CHAR(3)  NOT NULL REFERENCES stations(crs),
    -- Fare class from National Fares Manual (e.g. 'SDS'=Super Off-Peak Day Single).
    fare_class      TEXT     NOT NULL,
    price_pence     INTEGER  NOT NULL CHECK (price_pence >= 0),
    valid_from      DATE     NOT NULL,
    -- NULL means the fare has no scheduled end date.
    valid_to        DATE,
    PRIMARY KEY (origin_crs, destination_crs, fare_class, valid_from)
);

-- Hot query: cheapest fare between two stations valid today.
CREATE INDEX IF NOT EXISTS fares_od_idx ON fares(origin_crs, destination_crs);

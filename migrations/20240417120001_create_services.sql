-- Tier A: recurring rail service identities.
-- Keyed by RTTI UID (e.g. 'C12345'), which is stable across operating days.
-- The RID changes daily; the UID does not. This table is the authoritative
-- source for which services run and on which days.

CREATE TABLE IF NOT EXISTS services (
    uid             CHAR(6)      PRIMARY KEY,
    origin_crs      CHAR(3)      NOT NULL REFERENCES stations(crs),
    destination_crs CHAR(3)      NOT NULL REFERENCES stations(crs),
    -- Days bitmask: bit 0 = Monday, bit 6 = Sunday.
    -- 0b1111111 = 127 = runs every day.
    -- 0b0011111 = 31  = Mon–Fri only.
    runs_on_days    SMALLINT     NOT NULL DEFAULT 127
                                 CHECK (runs_on_days BETWEEN 0 AND 127),
    updated_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS services_origin_idx      ON services(origin_crs);
CREATE INDEX IF NOT EXISTS services_destination_idx ON services(destination_crs);

-- Phase 2: allow persisting uid → toc straight from the Darwin `schedule` message,
-- without a full timetable row. The schedule gives TIPLOCs (not 3-letter CRS), so
-- origin/destination can't satisfy the CRS foreign key — make them nullable. NULL
-- satisfies the FK, and the GTFS ingest still fills them when it runs. This lets
-- delay_history (keyed on uid) become operator-attributable via services.toc.

ALTER TABLE services ALTER COLUMN origin_crs DROP NOT NULL;
ALTER TABLE services ALTER COLUMN destination_crs DROP NOT NULL;

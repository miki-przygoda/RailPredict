# imports/

Static **reference-data exports** loaded into RailPredict's Tier A tables. These are the
official **Rail Settlement Plan (RSP) / Rail Delivery Group** reference files — the same
data that backs fares, station naming, and operator codes across the UK retail network. They
supersede the earlier OpenStreetMap-derived `uk-stations.zip`: the RSP exports carry the
official NLC and CRS codes, validity windows, and operator/ticket reference tables that the
OSM extract never had.

## Format note

All five files are **headerless** CSVs in raw RSP export format — column meaning is by
position, not by name. They are committed **as-is** (no reshaping); the loader maps columns
by index per the tables below. Date columns are `YYYY-MM-DD HH:MM:SS`; an open-ended
validity end is encoded as `2999-12-31`. Quoted fields (addresses, some names) may contain
commas, so parse with a real CSV reader, not a naive split.

## Files

| File | Rows | Cols | Target table | Maps to |
|------|------|------|--------------|---------|
| `rds_station.csv` | 7,328 | 24 | `stations` | Station master — name, NLC, CRS, validity |
| `rds_station_coords.csv` | 3,192 | 8 | `stations` (coords) | Lon/lat + postal address, joined by 4-digit NLC |
| `rds_toc.csv` | 80 | 6 | `operators` | Train operating companies — ATOC code → name |
| `rds_railcard.csv` | 290 | 9 | _(reference)_ | Railcards / discount-card types |
| `rds_ticket_type.csv` | 3,361 | 9 | _(reference)_ | Ticket / fare type codes |

### `rds_station.csv` — station master (24 cols)
The authoritative station list. 5,703 of the 7,328 rows carry a CRS (col 8); the remainder
are NLC-only settlement/group entries with no public CRS.

| Col | Field | Example |
|-----|-------|---------|
| 0 | row id | `1` |
| 1 | name | `Abbey Wood` |
| 2 | name, padded to 16 chars | `ABBEY WOOD      ` |
| 3 | short name | `Abbey Wood` |
| 4 | 7-digit NLC | `7051310` |
| 5 | 6-digit NLC | `705131` |
| 6 | **4-digit NLC** (join key) | `5131` |
| 7 | **CRS** (3-letter) | `ABW` |
| 9–10 | validity from / to | `2026-05-27 … / 2999-12-31 …` |

### `rds_station_coords.csv` — coordinates (8 cols)
Joins to `rds_station.csv` on the **4-digit NLC** (col 1 here = col 6 there). Note the
ordering: **longitude precedes latitude**. A handful of rows are `0,0` (no fix) and should be
skipped on load.

| Col | Field | Example |
|-----|-------|---------|
| 0 | row id | `1` |
| 1 | 4-digit NLC (join key) | `5131` |
| 2 | **longitude** | `0.12141` |
| 3 | **latitude** | `51.49107` |
| 4 | postal address (quoted) | `"Abbey Wood (London) Rail Station, …"` |

### `rds_toc.csv` — train operating companies (6 cols)
80 ATOC operator codes → operator names; backs the `operators` table and the operator
labels in the analytics/explorer pages.

| Col | Field | Example |
|-----|-------|---------|
| 0 | row id | `1` |
| 1 | **ATOC TOC code** (2-letter) | `AR` |
| 2 | name | `ANGLIA RAILWAYS TRAIN SERVICES` |
| 3 | logo filename | `AR.png` |

### `rds_railcard.csv` — railcards / discounts (9 cols)
290 railcard / discount-card definitions (code, name, discount, validity). Reference data for
a future fares/discount surface — not yet wired to a table.

| Col | Field | Example |
|-----|-------|---------|
| 0 | row id | `1` |
| 1 | railcard code | `WEB` |
| 2 | name | `11% WEB DISC` |
| 7–8 | validity from / to | `2015-09-08 … / 2999-12-31 …` |

### `rds_ticket_type.csv` — ticket types (9 cols)
3,361 fare/ticket type codes (e.g. `0AA` → "Smart Child Flat Fare Single"). Reference data
for fare classification — not yet wired to a table.

| Col | Field | Example |
|-----|-------|---------|
| 0 | row id | `1` |
| 1 | ticket type code | `0AA` |
| 2 | description | `Smart Child Flat Fare Single` |
| 5–6 | validity from / to | `2019-07-17 … / 2999-12-31 …` |

## Notes

- These supersede the OSM `uk-stations.zip` (removed): the RSP exports add the official
  NLC/CRS codes and validity windows previously flagged as missing in `docs/tech-debt.md` §C1.
- Scope is **Great Britain** National Rail retail data; Northern Ireland is a separate network.
- `rds_railcard.csv` and `rds_ticket_type.csv` are loaded as reference only for now — the
  fares/discount surface that consumes them is not yet built.

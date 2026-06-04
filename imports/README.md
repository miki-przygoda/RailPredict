# imports/

Static **reference-data archives** loaded into RailPredict's Tier A tables. Each archive
is a small, self-documenting zip (a CSV plus a README describing its columns and provenance).

| Archive | Contents | Target table | Status |
|---------|----------|--------------|--------|
| `uk-stations.zip` | All GB National Rail stations — `crs`, `name`, `lat`, `lon`, `country` (2,605 stations) | `stations` | ✅ added |
| _(railcards / discounts)_ | The railcard / discount-card types a rail user might hold | _tbd_ | ⏳ to be added |

Notes:
- Station rows carry CRS + coordinates but **not** official NLC / TIPLOC codes — those
  arrive with the GBR / company reference feed (see `docs/tech-debt.md` §C1).
- Scope is **Great Britain** National Rail; Northern Ireland is a separate network.

# Darwin Data Audit — what we collect, what we drop, what we can infer

**Date:** 2026-06-05
**Status:** Analysis only (no app changes). Grounds the "derivable signals" roadmap.
**Sources (code):** `ingestion/parser.rs`, `ingestion/filter.rs`, `types/train_status.rs`,
`prediction/types.rs`, `db/history.rs`, `db` schema.

---

## TL;DR

A single Darwin message is **dense**; we distil it to a **single number** (the origin's
departure delay). Three compounding losses:

1. **At the filter:** `schedule (SC)` and `SF (formations)` pass the route filter but the
   parser has **no arm for them**, so we receive them and extract **nothing**. `OW`
   (incident/disruption messages), `trainAlert`, `alarm` are dropped outright.
2. **At the parser (TS):** we read only the **first** `Location`'s `ptd/wtd/plat/tpl` and a
   `dep`'s `et/at/delayed`, plus the last `Location`'s `tpl` as the destination. **No
   arrival times, no intermediate stops, no reason codes, no platform-confidence flags.**
3. **At persistence:** `delay_history` stores `(uid, weekday, origin_crs, departure_hour,
   delay_mins, predicted_delay_mins, recorded_at)`. Everything richer is computed live for
   the model, then **discarded** when the train is evicted.

The biggest single unlock — **parse the `schedule` message** — costs us nothing in feed
volume (we already receive it) and yields `toc` (the operator-league blocker), train
category, the full planned calling pattern, and activities.

---

## 1. Message-type taxonomy: offered vs handled

| Darwin type | Filter decision | Parser arm? | What it carries | What we do |
|---|---|---|---|---|
| `TS` (train status) | KEEP | ✅ partial | Live forecast/actual times, platform, cancel, per-stop progress | Parse origin dep + dest tpl only |
| `deactivated` | KEEP | ✅ | Train terminated/cancelled | → Terminal state |
| `Association` (NP) | CONDITIONAL | ✅ | Turnround: prev RID → next RID (same stock) | Predecessor-delay feature |
| **`schedule` (SC)** | CONDITIONAL | ❌ **none** | **`toc`, trainCat, power type, full plan (every call w/ planned times + activities), divide/join, cancel reason** | **Received, parsed to nothing** |
| **`SF` (formations)** | CONDITIONAL | ❌ **none** | Coach/unit formation per stop | **Received, parsed to nothing** |
| `fL` (formation loading) | (DROP/default) | ❌ | **Per-coach loading %** (crowding) | Dropped |
| `OW` (station message) | DROP | ❌ | **NRCC incident/disruption free-text + severity** | Dropped |
| `trainAlert` | DROP | ❌ | Service alerts | Dropped |
| Other assoc (JJ join, VV divide) | (default DROP) | ❌ | Coupling/splitting | Dropped |
| `alarm`, `trackingID`, `trainOrder` | DROP/default | ❌ | NR internal / platform order | Dropped |

> `Conditional` in the filter means "reaches the parser"; it does **not** mean the parser
> understands it. `schedule`/`SF` reach `parse_pport` and match no element arm.

## 2. One `TS` row: full inventory vs what we extract

A `TS` is `<TS rid ssd uid can>` wrapping a list of `<Location tpl wta wtd wtp pta ptd plat>`,
each optionally containing `<arr>`, `<dep>`, `<pass>` forecast elements with `et` (estimated),
`at` (actual), `atRemoved`, `src` (forecast source), and `delayed`; plus `<plat>` attributes
`platsup`/`platsrc`/`conf`, a `length`, and late/cancel **reason codes**.

| Field (per location/stop) | In a TS? | We extract? | Notes |
|---|---|---|---|
| `tpl` (TIPLOC of stop) | ✅ every stop | first + last only | intermediate stops dropped |
| `ptd`/`pta` (public sched dep/arr) | ✅ | dep, first only | no arrival, no intermediate |
| `wtd`/`wta`/`wtp` (working sched) | ✅ | `wtd` first only | working arr/pass dropped |
| `dep et/at` (est/actual departure) | ✅ | ✅ (last `dep` seen) | overwritten per `dep` element |
| **`arr et/at` (est/actual arrival)** | ✅ | ❌ | **no arrival delay anywhere** |
| **`pass et/at`** | ✅ | ❌ | no passing-point progress |
| `delayed` flag | ✅ | ✅ | boolean only |
| **late/cancel reason code** | ✅ (often) | ❌ | **"why" is thrown away** |
| `plat` value | ✅ | ✅ (first) | string only |
| **`platsup`/`platsrc`/`conf`** | ✅ | ❌ | no platform-confidence |
| **`length` / formation** | ✅ (TS or SF) | ❌ | no train-length signal |
| `can` (whole-service cancel) | ✅ | ✅ | per-stop `can` (part-cancel) dropped |

**Net:** from a whole-journey message we keep **origin departure delay + cancel flag +
destination CRS**. We have **zero arrival data** — yet arrival at the destination is what
passengers actually care about.

## 3. What we persist vs compute-then-discard

- **Persisted (`delay_history`):** `uid, weekday, origin_crs, departure_hour, delay_mins,
  predicted_delay_mins, recorded_at`. One row = one origin-departure observation.
- **Computed live then discarded** (held only in `TrainStatus` / the model feature vector
  until eviction): platform (sched+actual), estimated vs actual departure, the calling-point
  list, predecessor/turnround link, and the whole `LiveFeatures` block below.

## 4. What we ALREADY derive (credit where due)

The Tier-B engine already computes (per `prediction/types.rs`), it just doesn't **persist or
surface** most of it:

- **`RollingStats` (7d & 14d):** mean delay, **std-dev of delay** (← the stochastic
  component), on-time %, sample-count(log). The raw material for a reliability score exists.
- **`LiveFeatures` (8):** current delay; preceding-service delay (same origin);
  `station_congestion_30m` (mean delay of other trains at the origin); `operator_cascade_delay`
  (mean delay of trains sharing the **UID first-letter** as a crude operator proxy);
  `predecessor_train_delay` (NP turnround); wind mph; volatility score (none/wind/incident/both);
  mins-until-departure.

So congestion, a turnround signal, a weather/incident signal, and a variance measure already
exist — but as ephemeral model inputs, not as stored or user-facing intelligence.

## 5. Inference opportunities (three tiers)

### Tier 1 — parse what we already receive (near-zero feed cost)
| Signal | Source (already arriving) | Unlocks |
|---|---|---|
| **Operator (`toc`)** | `schedule` message | The operator league/drill-downs (current prod-handoff blocker) — no GTFS needed |
| **Train category / power** | `schedule` | Express vs stopper vs freight segmentation |
| **Full planned calling pattern + activities** | `schedule` | Per-segment analysis, request-stops, set-down-only |
| **Late / cancel reason codes** | `TS` reason code | **"Why late" attribution** — signalling/weather/fleet/trespass; the single richest signal |
| **Arrival / destination delay** | `TS` `<arr at/et>` | Arrival punctuality (what passengers want) — today we only have departure |
| **Platform confidence** | `TS` `platsup/platsrc/conf` | "Usually P4 (confirmed)" + change alerts |
| **Per-coach loading** | `fL` (currently dropped) | Crowding by service/time |
| **Partial cancellation** | per-`Location` `can` | "Runs but skips your stop" ≠ cancelled |

### Tier 2 — persist the fuller journey (schema work)
Store more than one number per service: **arrival delay**, **per-stop delay vector**, and
**actual vs scheduled dwell**. This turns the discard pile into queryable history and unlocks
Tier 3 retrospectively.

### Tier 3 — derived signals (pure functions over Tier 1–2)
| Signal | Derived from | Value |
|---|---|---|
| **Reliability score (systematic vs stochastic)** | mean (already have) + std (already have) | "reliably 3 min late" vs "wildly variable" — honest beyond on-time% |
| **Delay trajectory / recovery rate** | per-stop delay vector | recovering vs deteriorating; mins recovered per stop |
| **Per-segment delay attribution** | Δdelay between stops | *where* delay is incurred (the hotspot section) |
| **Dwell anomaly** | actual vs scheduled dwell | boarding congestion per station |
| **Peak-degradation curve** | delay × hour aggregation | quantified "peak penalty" per route/station |
| **Delay persistence (autocorrelation)** | same service day-over-day | does yesterday predict today? |
| **Connection risk** | inbound delay distribution + interchange dwell | P(miss the connection) — genuinely novel |
| **Junction/infrastructure contention** | shared TIPLOC ±window (`check_tiploc_cascade`, staged) | knock-on across different services |
| **Reason-conditioned prediction** | reason code (Tier 1) × history | "weather delays recover; signalling delays don't" |

## 6. Recommended priority

1. **Parse `schedule`** → `toc` + category + plan. Highest leverage, zero extra feed; fixes
   the operator league and feeds segmentation. (Note: this is the *correct* per-service
   `uid→toc`, unlike the location-managing-TOC we deliberately avoided.)
2. **Parse `TS <arr>` + reason codes** → arrival delay + "why". Two attributes; transformative.
3. **Persist the journey profile** (arrival + per-stop vector + dwell) → unlocks Tier 3.
4. **Reliability score** from existing mean/std → a flagship metric with no new ingestion.

All of the above are pure data/logic work — none require changing the live-prediction hot path.

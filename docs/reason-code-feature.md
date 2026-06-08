# Feature flag — Darwin reason codes + "earned vs coincidental" accuracy

**Status:** PROTOTYPE only. Lives in `RailPredict/tests/reason_code_prototype.rs` — NOT
wired into the app. (Placed in `tests/` rather than `data/` because `/data` is gitignored;
this doc lives in `docs/` for the same reason.)

**Run it:** `cargo test --test reason_code_prototype -- --nocapture`

**Background:** `docs/darwin-data-audit.md` (Tier 1: "parse reason codes"; Tier 3:
"reason-conditioned prediction").

---

## What it is

Darwin `TS` / `schedule` messages can carry a numeric **reason code** explaining why a
service is late or cancelled (signalling, trespass, weather, congestion, …). We don't parse
these today. The prototype:

1. **Parses** reason codes from a Darwin fragment (element form `<LateReason>163</LateReason>`,
   self-closing `<CancelReason code="100"/>`, and attribute form `delayReason="168"`),
   yielding `{ kind: Late|Cancel, code, tiploc?, near }`.
2. **Classifies** each reason as **Structural** (recurring/operational — the model *can* learn
   it: congestion, awaiting platform, regulation, connections) or **Exogenous** (a one-off
   incident the model could never have known: trespass, broken rail, person hit, severe weather).
3. **Interrogates accurate predictions.** A within-±5-min prediction looks good — but *why* was
   it accurate? The classifier returns a verdict:

   | Verdict | When | Meaning |
   |---|---|---|
   | `EarnedAccurate` | ±5, structural | model captured a real recurring cause — **robust** |
   | `CoincidentalAccurate` | ±5, exogenous | model got **lucky** on an unmodellable incident |
   | `ExplainedMiss` | miss, exogenous | understandable surprise, not the model's fault |
   | `ModelError` | miss, structural | a genuine miss the model should have caught |
   | `CleanAccurate` / `UnexplainedMiss` | no reason on record | — |

## What the prototype shows

```
  pred  act  err  reason                  verdict                why
     8    9    1  congestion              EarnedAccurate         recurring — robust hit
     6    7    1  trespass                CoincidentalAccurate   one-off incident — lucky
    10   11    1  severe weather          CoincidentalAccurate   one-off incident — lucky
     5   35   30  broken rail             ExplainedMiss          unmodellable incident
     2   20   18  congestion (missed)     ModelError             should have learned this
  Of the accurate (<=5) hits: 2 earned, 2 coincidental.
```

The insight: **a headline "within ±5 min %" conflates robust accuracy with luck.** Splitting it
by reason class tells you how much of your accuracy is *earned* (you're modelling real causes)
vs *coincidental* (you're getting lucky on incidents you never modelled) — and reframes "misses"
caused by genuine incidents as not-the-model's-fault.

## What it will become (when wired in)

1. **Ingestion:** add a `LateReason`/`CancelReason` arm to `ingestion/parser.rs`, extend
   `TsUpdate` with an optional `reason: { kind, code, tiploc, near }`. (`schedule`/`TS` already
   reach the parser — see the audit.)
2. **Reference:** replace the illustrative code map with the official Darwin
   **LateRunningReasons / CancellationReasons** reference; keep the Structural/Exogenous
   classification as a small curated overlay.
3. **Persist:** add `reason_code` (+ derived class) to `prediction_outcomes`, captured at
   finalisation alongside `final_delay_mins`.
4. **Surface:**
   - A **"why late"** chip on the train-detail page and the live board.
   - On `/predictions`, an **earned-vs-coincidental breakdown** of the within-±5 hits, plus a
     reason-conditioned accuracy view (and the option to *exclude* exogenous incidents from the
     headline accuracy metric, so the model isn't blamed for unmodellable events).
5. **Model (stretch):** reason-conditioned recovery — weather/congestion delays tend to recover,
   signalling/infrastructure delays persist — feeding genuinely reason-aware predictions.

## Why it's worth doing

- Turns "% accurate" from a vanity number into an **honest, decomposed** one.
- Adds a **"why"** layer nothing else surfaces well.
- Costs nothing in feed volume — the reason codes are already arriving in messages we receive.

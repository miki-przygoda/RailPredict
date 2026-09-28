# ML Delay Prediction -- Model Performance

**Status: no valid accuracy figures are currently published.**
**Last updated:** 28 September 2026 (v1.21.1)
**Models:** LightGBM, exported as ONNX -- `models/day_ahead.onnx`, `models/realtime.onnx`
**Training script:** `scripts/compare_models.py` (v8)

---

## Withdrawn results

Every MAE previously published for these models -- in this file, `models/benchmarks.json`,
the README, the generated `docs/*.html` reports and the CHANGELOG (e.g. real-time MAE
3.97 / 4.09 / 4.41 min, day-ahead 13.10-14.39 min) -- is **withdrawn**. They are not
valid estimates of prediction accuracy, for three independent reasons:

1. **Target leakage (real-time model).** The `current_delay_mins` training feature was
   built as `delay_mins * U(0.7, 1.3)`: the label itself plus multiplicative noise. The
   model learned to read the answer back from its input. This was in both
   `compare_models.py` and the older `train_models.py`; the synthetic generator did the
   same with `U(0.85, 1.15)`.
2. **Random split on time-series data.** The test set was a "15% random stratified
   split". Snapshots of the same journey, and observations minutes apart at the same
   station, landed on both sides, so the test set was not unseen data. Rolling features
   also included the current run's own earlier snapshots, and encodings/operator means
   were fitted on the full frame (test included).
3. **Invalid labels.** Until the v1.15.3 fix (05/06/2026), `delay_history.delay_mins`
   measured journey progress (time since origin departure), not origin delay (median
   ~21-32 min vs a true median of 0). All published benchmarks were trained on
   late-May / 1 June 2026 data, before that fix.

The committed `.onnx` files are from that pipeline. They keep the 14/22-feature layout
the Rust engine expects, so the runtime still loads them, but their accuracy is unknown
until they are retrained. The live `/predictions` page scores logged predictions against
real outcomes and is not affected by this retraction.

---

## First v8 run (28/09/2026) -- not shipped

Run on the local post-fix corpus (`railpredict_v2`, 1.93M rows, 5-10 June 2026), default
settings (no synthetic rows, equal-tier weights). Split: fit 5-8 June, validation 9 June,
test 10 June (one partial day, 188k rows).

| Model | Test MAE | Bias | Baseline MAE |
|---|---|---|---|
| Day-ahead (14 features) | 7.36 min | +4.48 | trimmed mean: 2.09 min |
| Day-ahead HC (3k trees) | 6.76 min | +4.62 | |
| Real-time (22 features) | 5.16 min | +4.60 | persistence: 1.02 min |
| Real-time HC (3k trees) | 4.68 min | +3.01 | |

**Both models lose to their trivial baselines**, so these models were not committed and the
previous `.onnx` files remain in place (accuracy unknown, see above). Likely causes:

- *Equal-tier sample weighting* (severe delays weighted ~18x) was designed for the pre-fix
  data, where most rows looked severely delayed. On correct labels (median 0) it drags every
  prediction upward, hence the ~+4.5 min bias.
- *Five days of data.* Rolling features are near-empty: a pattern includes the weekday, so a
  7-day window has at most one prior run, which the corpus does not contain yet.
- The Rust side adds `DAY_AHEAD_BIAS_MINS = 6` to day-ahead output, calibrated to the old
  models; it must be revisited with any retrain.

Any fix to weighting or features must be chosen on the validation dates, not on the test
day, before numbers are published.

---

## Evaluation method (v8, current)

| Aspect | Now |
|---|---|
| Label floor | `--since 2026-06-05` (post label fix) by default |
| Split | Date-ordered: last ~15% of service dates = test, the ~10% before = early-stopping validation, the rest = fit. No journey straddles a boundary. |
| Real-time rows | Non-final snapshots of each journey (pattern + UTC service date) |
| Real-time target | The journey's **final** observed origin delay |
| `current_delay_mins` | The snapshot's own Darwin reading (what the engine sees at prediction time) |
| Real-time baseline | Persistence: predict final = current reading |
| Day-ahead baseline | Per-pattern trimmed mean |
| Rolling stats | Prior service dates only (same in Rust, `HistoricalStore::rolling_stats_7d`) |
| Encodings / operator means | Fitted on the fit dates only; unseen values -> 0 (OOV), as at inference |
| Synthetic rows | Off by default (`--synthetic`); if enabled, day-ahead fit set only, never in validation, test or the real-time model |

`models/benchmarks.json` is only rendered by the export report when it carries
`"status": "valid"`, which only `compare_models.py` v8+ writes.
`scripts/test_compare_models.py` holds DB-free leakage regression tests
(`python -m unittest scripts/test_compare_models.py`).

---

## Architecture

### Two-model cascade

```
Darwin TS message arrives
  |
  |- reported_delay_mins available?
  |     YES -> realtime.onnx (22 features)
  |     NO  -> day_ahead.onnx (14 features)
  |
  '- ONNX returns None (model not loaded)?
        -> trimmed-mean statistical fallback (needs >=3 historical samples)
```

### Day-ahead model -- 14 features

`weekday`, `departure_hour`, `month`, `is_peak`, `origin_crs_enc`, `uid_prefix_enc`,
`rolling_mean_7d`, `rolling_std_7d`, `rolling_ontime_7d`, `sample_count_log`,
`rolling_mean_14d`, `rolling_std_14d`, `weekday_operator_enc`, `operator_relative_delay`.

### Real-time model -- 22 features (day-ahead + 8)

`current_delay_mins`, `preceding_delay_mins`, `wind_mph`, `volatility_score`,
`mins_until_departure`, `station_congestion_30m`, `operator_cascade_delay`,
`predecessor_train_delay`.

The order is fixed and must match `onnx_engine.rs` (`N_DAY_FEATURES`, `N_RT_FEATURES`).

### Statistical fallback

- Trimmed mean of the pattern's historical records (trims top/bottom 10%), >=3 samples
- Confidence decays as `confidence * exp(-days / 21)` when the newest sample is >21 days old
- Blends in preceding-service delay at the same origin (0.6/0.4) within +/-20 min

---

## Known limitations

1. **No shippable model yet.** The first leak-free run lost to trivial baselines (see above).
2. **Real-time training rows need multi-snapshot journeys.** `record_outcome` writes on a
   >=2 min change or every 5 min, so on-time trains that Darwin stops updating produce few
   snapshots. The "on-time heartbeat" task in `main.rs` that was meant to fill this gap
   listens for same-state poll events that nothing currently emits.
3. **Snapshots after departure.** Once a train has left its origin, its origin delay is
   final, so later snapshots are trivially predictable. They are kept (the engine also
   predicts for en-route trains), which flatters both the model and the persistence
   baseline; compare the model against that baseline, not in isolation.
4. **`predecessor_train_delay` is a proxy in training.** Training has no Association
   turnround data, so it copies `preceding_delay_mins`; at inference it comes from Darwin
   NP associations. This is train/serve skew, not leakage.
5. **Weather is a single anchor.** Training wind comes from the Open-Meteo archive at
   Heathrow (hourly); if the fetch fails the features are zero.
6. **`month` carries no signal** until the data spans several months.
7. **`uid_prefix` encodes TOC, not route.**
8. **Journey key uses the UTC date**, so a run that crosses UTC midnight splits in two.

---

## Retraining

```bash
make train                   # labels from 2026-06-05 onwards
make train SINCE=2026-07-01  # later floor
```

Outputs `models/day_ahead.onnx`, `models/realtime.onnx`, `models/feature_meta.json` and
`models/benchmarks.json`; restart the server to load them. The training corpus stays
internal (`delay_history`); it is not distributed.

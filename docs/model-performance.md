# ML Delay Prediction — Model Performance

**Last evaluated:** 30 May 2026  
**Data source:** Live Darwin Push Port feed (UK National Rail)  
**Models:** LightGBM v3, exported as ONNX — `models/day_ahead.onnx`, `models/realtime.onnx`  
**Training script:** `scripts/compare_models.py`

---

## Current Results (v3 — 30 May 2026)

Trained on **3,036,926 real Darwin observations** (21–30 May 2026, bad days excluded).  
Test set: 15% random stratified split (455,539 rows).

| Model | MAE | RMSE | Bias | ±2 min | ±5 min | ±10 min |
|---|---|---|---|---|---|---|
| **Day-ahead** (10 features) | **14.12 min** | 21.75 min | −6.34 min | 14.2% | 32.1% | 54.3% |
| **Real-time** (15 features) | **4.09 min** | 6.42 min | −0.15 min | 42.9% | 72.5% | 90.3% |
| Trimmed-mean baseline | 17.98 min | — | — | — | — | — |

**vs. previous models (trained 28 May on ~1.4M rows):**

| Model | Previous MAE | Current MAE | Delta |
|---|---|---|---|
| Day-ahead | 17.59 min | 14.12 min | ▼ 3.47 min |
| Real-time | 5.92 min | 4.09 min | ▼ 1.83 min |

---

## Architecture

### Two-model cascade

The engine tries models in order, falling back if a model isn't loaded or an encoding is unknown:

```
Darwin TS message arrives
  │
  ├─ reported_delay_mins available?
  │     YES → realtime.onnx (15 features)
  │     NO  → day_ahead.onnx (10 features)
  │
  └─ ONNX returns None (unknown TIPLOC or model not loaded)?
        → trimmed-mean statistical fallback (needs ≥3 historical samples)
```

### Day-ahead model — 10 features

Used when a train hasn't yet produced a live Darwin delay reading (pre-departure, dormant/monitored state).

| # | Feature | Source | Importance |
|---|---|---|---|
| 0 | `origin_crs_enc` | TIPLOC label-encoded from `feature_meta.json` | ████████████████████████████ |
| 1 | `rolling_std_7d` | 7-day std dev of delay for this service pattern | ██████████████████████ |
| 2 | `rolling_mean_7d` | 7-day mean delay for this service pattern | █████████████████████ |
| 3 | `sample_count_log` | log1p(n), where n = 7-day sample count | ████████████████████ |
| 4 | `rolling_ontime_7d` | % of services on time in last 7 days (0–100) | ███████████████ |
| 5 | `departure_hour` | Planned departure hour 0–23 | ██████████████ |
| 6 | `uid_prefix_enc` | First char of UID (proxy for train operating company) | ██████████ |
| 7 | `weekday` | 0 = Monday … 6 = Sunday | ██████ |
| 8 | `is_peak` | Hour ∈ {7,8,16,17,18} ∧ weekday ≤ 4 | █ |
| 9 | `month` | 1–12 | — (zero — insufficient seasonal range) |

### Real-time model — 15 features (day-ahead + 5)

Activated once the Darwin feed reports a live delay for an active train.

| # | Feature | Source | Importance |
|---|---|---|---|
| 0 | `origin_crs_enc` | TIPLOC label-encoded | ████████████████████████████ |
| 1 | `current_delay_mins` | Darwin reported delay (trained with ±30% noise) | ███████████████████████████ |
| 2 | `mins_until_departure` | Scheduled departure − now | █████████████████████████ |
| 3 | `rolling_mean_7d` | 7-day mean delay | ████████████████████████ |
| 4 | `rolling_std_7d` | 7-day std dev of delay | ████████████████████████ |
| 5 | `sample_count_log` | log1p(7-day sample count) | ███████████████████ |
| 6 | `rolling_ontime_7d` | % on time last 7 days | ███████████████ |
| 7 | `preceding_delay_mins` | Most recent service at same origin within ±20 min | █████████████ |
| 8 | `departure_hour` | Planned departure hour 0–23 | █████████████ |
| 9 | `uid_prefix_enc` | TOC proxy from UID prefix | ████████ |
| 10 | `weekday` | 0–6 | ████ |
| 11 | `is_peak` | Peak hour flag | █ |
| 12 | `month` | 1–12 | — (zero — insufficient seasonal range) |
| 13 | `wind_mph` | Open-Meteo weather anchor | — (zero — WEATHER_ANCHORS not configured) |
| 14 | `volatility_score` | 0–3 wind/incident composite | — (zero — WEATHER_ANCHORS not configured) |

**Training note:** `current_delay_mins` was multiplied by U(0.7, 1.3) noise during training to stop the model simply copying the input and force integration of pattern context with the live signal.

### Statistical fallback

When neither ONNX model fires (e.g. TIPLOC not in training vocabulary):
- Trimmed mean of the service pattern's historical records (trims top/bottom 10%)
- Requires ≥3 samples; emits no prediction otherwise
- Confidence decays exponentially if the most recent sample is >21 days old: `confidence × exp(−days / 21)`
- Blends in preceding-service delay at the same origin (0.6/0.4 weight) when a correlated service is found within ±20 min

---

## Training Data

| Property | Value |
|---|---|
| Source | Live Darwin Push Port feed (real observations) |
| Training window | 21–30 May 2026 |
| Total rows (after filter) | 3,036,926 |
| Excluded days | 2026-05-21, 2026-05-27 (startup reconnect artifacts — massive negative delays) |
| Delay filter | `delay_mins BETWEEN -30 AND 240` |
| Unique TIPLOCs | 2,662 stations in `feature_meta.json` |
| UID prefix encodings | 18 TOC prefixes |
| Train / test split | 85% / 15% random stratified by delay tier |

### Delay tier distribution (training set)

| Tier | Definition | Share | Real-world UK % |
|---|---|---|---|
| Severe | > 30 min | 53.0% | ~2% |
| Moderate | 6–30 min | 32.2% | ~5% |
| On-time | ≤ 0 min | 10.4% | ~85% |
| Slight | 1–5 min | 4.4% | ~8% |

The Darwin feed generates a training record every time a train's delay changes by ≥2 minutes, but only every ~5 minutes for on-time trains. This produces severe overrepresentation of delayed trains. Equal-tier sample weighting is applied during training so each tier contributes 25% of the total gradient weight.

### Feature engineering notes

- **`preceding_delay_mins`**: computed via `pandas.merge_asof` — for each record, the most recent observation at the same TIPLOC within the prior 20 minutes with a different UID. **66% of rows have a non-zero predecessor** (mean predecessor delay: 32.1 min).
- **`mins_until_departure`**: approximated from `departure_hour` and `recorded_at`. Range: −360 to +360 min, mean: +49.4 min.

---

## Known Limitations

**1. Dataset is 10 days old — no seasonal signal yet**  
`month` has zero feature importance because all data falls within a single month (May 2026). Models will not generalise to winter timetable changes, engineering works, or bank holiday patterns until the dataset spans multiple months. Rolling 7-day features partially compensate within the current window.

**2. 28 May dominates the dataset**  
1.34M of 3.04M rows (44%) are from a single day. This means gradient steps are disproportionately shaped by that day's conditions. A per-day row cap would reduce this dominance. Planned fix in a future training run.

**3. Bad day filter has a timezone edge case**  
`recorded_at::date NOT IN ('2026-05-21', '2026-05-27')` uses the timestamp's local timezone for the date cast. BST (+01:00) means some records near midnight land on the wrong date. 21 May and 27 May still show ~11K rows each in training despite being listed as excluded. Fix: cast to UTC before date comparison.

**4. Day-ahead model has a −6.34 min bias**  
The day-ahead model systematically underestimates delays. This is partly a consequence of the imbalanced training distribution (severe delays overrepresented despite sample weighting). Will improve as the dataset grows and `month` starts carrying seasonal signal. A post-inference +6 min offset is a short-term mitigation option.

**5. Weather features are zeros without `WEATHER_ANCHORS` configured**  
`wind_mph` and `volatility_score` are only non-zero if `WEATHER_ANCHORS` is set in the environment. Without it, the real-time model uses pattern + current delay only. Configuring even one anchor (e.g. London) would activate these features.

**6. `uid_prefix` encodes TOC, not route**  
The first character of the UID identifies the train operating company — a proxy for route characteristics but cannot distinguish individual routes within a TOC. Route-level or headcode encoding would give finer granularity.

**7. No coverage below 3 samples per pattern**  
The statistical fallback emits no prediction for patterns with fewer than 3 historical records. New TIPLOCs entering the feed will have zero coverage until records accumulate.

---

## Retraining

As live Darwin data accumulates, retrain with:

```bash
cd scripts
python3.11 compare_models.py
# Outputs: ../models/day_ahead.onnx, ../models/realtime.onnx, ../models/feature_meta.json
# Then restart the server to load new models
```

Models are picked up from `models/` at server startup — no recompilation needed.

Retraining takes ~5–10 minutes on a laptop at 3M+ rows (LightGBM, 1500 estimators, 127 leaves). Accuracy improves naturally as the dataset grows and begins to cover multiple months, routes, and seasonal patterns.

The training corpus is held internally in the `delay_history` table (and the frozen `railpredict` corpus DB); it is not distributed.

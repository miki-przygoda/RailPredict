# Model Improvement Plan
**Status:** In progress — created 2026-05-29

---

## Current State

| Metric | Value | Grade |
|---|---|---|
| Within ±5 min | 54.6% | Moderate |
| Within ±10 min | 70.3% | Moderate |
| Avg prediction error | 12.3 min | Poor |
| On-time rate in training data | ~11% | Massively biased |

### Root causes

**1. Severe training data bias**
The write throttle causes 53% of training rows to be "severe delay" (>30 min):

| Tier | DB rows | % of data | Real-world UK % |
|---|---|---|---|
| On-time (≤0 min) | 301K | 10.9% | ~85% |
| Slight (1–5 min) | 127K | 4.6% | ~8% |
| Moderate (6–30 min) | 881K | 31.9% | ~5% |
| Severe (>30 min) | 1.46M | **52.7%** | ~2% |

Why: delayed trains generate a record every time delay changes ≥2 min (frequent). On-time trains only record once every 5 min. Result: the model has almost never trained on a "train is running fine" example. MAE on on-time trains = 20.4 min.

**2. 28 May dominates the dataset (55% of all rows)**
1.5M rows on a single day vs 65–230K on every other day. Any gradient step touches 28 May heavily, pulling weights toward that day's pattern.

**3. Only 7 days of real data**
No seasonality, no winter weather signal, no bank holiday patterns, very limited route diversity.

**4. Date-based train/test split leaks the bias**
Using "today" (29 May) as the test set means the test distribution matches the live data, but the training distribution is dominated by 28 May. A random split would ensure both sets see all kinds of delay days proportionally.

---

## Plan

### Step 1 — Pull real historical data via Network Rail HSP API

**Script:** `scripts/fetch_hsp_history.py`

**Source:** Network Rail Historic Service Performance (HSP) API
- Base URL: `https://hsp-prod.rockshore.net/api/v1/`
- Auth: HTTP Basic — same username/password as Darwin (`DARWIN_USERNAME` / `DARWIN_PASSWORD` from `.env`)
- Rate limit: ~1 req/s (be conservative, use 0.5 req/s)
- History available: up to 24 months back

**What we fetch:**
- Top 100 origin TIPLOCs from our existing `delay_history` (these are the stations the Darwin feed already covers, so we have UIDs to match against)
- Date range: past 12 months (configurable via `--months`)
- For each station × date: query `/serviceMetrics` to enumerate services, then `/serviceDetails` per service to get per-stop actual departure delay

**Schema mapping:**
```
HSP actual_td - HSP gbtt_ptd  →  delay_mins  (departure delay in minutes)
HSP location                  →  origin_crs
HSP rid                       →  uid  (strip date prefix to get base UID)
date + gbtt_ptd               →  weekday, departure_hour, recorded_at
predicted_delay_mins          →  NULL  (no predictions for historical records)
```

**Expected volume:** ~50–200 rows per station per day × 100 stations × 365 days ≈ 1.8M–7.3M new rows. Even the low end roughly doubles our dataset with REAL data distributed across all 12 months.

**Insertion:** `ON CONFLICT DO NOTHING` — idempotent, safe to re-run.

---

### Step 2 — Clean up existing data bias

**Script:** SQL run directly (no new file needed)

Two cleanups:

**2a. Cap 28 May to 300K rows (keep a random sample)**
```sql
-- Keep 300K random rows from 28 May, delete the rest
DELETE FROM delay_history
WHERE recorded_at::date = '2026-05-28'
  AND id NOT IN (
    SELECT id FROM delay_history
    WHERE recorded_at::date = '2026-05-28'
    ORDER BY random() LIMIT 300000
  );
```
This brings 28 May in line with other days and removes its outsized influence without discarding it entirely.

**2b. Cap severe-delay rows overall to 3× on-time rows**
After HSP data is loaded, the distribution should self-correct. If it doesn't, a second SQL pass will cap `delay_mins > 30` rows via random deletion.

---

### Step 3 — Fix the write throttle going forward

**File:** `RailPredict/src/prediction/engine.rs`

**Problem:** The current throttle (`MIN_DELAY_CHANGE_MINS=2, MIN_RECORD_INTERVAL_SECS=300`) works fine for delayed trains but causes on-time trains to be underrepresented because Darwin doesn't send TS messages for trains that haven't changed.

**Fix:** Call `record_outcome` on every successful state-machine poll result (not just on Darwin TS message arrival) for trains in `Active` or `Critical` state. This guarantees an on-time heartbeat record every poll cycle even when Darwin sends no update.

Specifically in `poll_manager.rs` / the poll callback: after updating `TrainStatus` from a live poll, unconditionally call `engine.record_outcome(&status)`. The existing throttle already handles deduplication — the new signal is that on-time trains now reliably generate a fresh `delay_mins=0` record on each poll, just like delayed trains generate one on each delay change.

**New constants (no change needed — existing throttle handles it):**
- On-time train at 0 delay: change=0, elapsed≥300s → records every 5 min ✓
- Delayed train changing: change≥2 → records immediately ✓
- The gap was that Darwin wasn't sending TS messages for on-time trains at all. Hooking into the poll loop closes that gap.

---

### Step 4 — Retrain with random split + sample weighting

**File:** `scripts/compare_models.py`

**4a. Random train/test split (replace date-based)**
Instead of `TEST_DATE = "2026-05-29"`, split randomly across all data with stratification by delay tier so every tier is proportionally represented in both halves.

```python
from sklearn.model_selection import train_test_split

# Stratify by delay tier to preserve tier distribution in both splits
df["_tier"] = pd.cut(df["delay_mins"], bins=[-999, 0, 5, 30, 9999], labels=["ontime","slight","moderate","severe"])
train, test = train_test_split(df, test_size=0.15, random_state=42, stratify=df["_tier"])
df.drop(columns=["_tier"], inplace=True)
```

Why 15% test: with ~4M+ rows after HSP data load, 15% is 600K rows — large enough to be statistically stable.

**4b. Sample weighting to correct the severe-delay bias**
Assign each training row a weight inversely proportional to its tier frequency, so the model sees all four tiers equally:

```python
tier_counts = train["delay_mins"].apply(
    lambda x: "ontime" if x <= 0 else "slight" if x <= 5 else "moderate" if x <= 30 else "severe"
).value_counts()

# Target: equal weight per tier (each tier contributes 25% of total weight)
total = len(train)
tier_weight = {t: (total / (4 * tier_counts[t])) for t in tier_counts.index}

weights = train["delay_mins"].apply(
    lambda x: tier_weight["ontime"] if x <= 0
              else tier_weight["slight"] if x <= 5
              else tier_weight["moderate"] if x <= 30
              else tier_weight["severe"]
).values
```

Pass `sample_weight=weights` to `m.fit(...)`.

**4c. Remove BAD_DAYS exclusion**
With HSP historical data in the mix, 21 May and 27 May are no longer big enough to distort training. Remove the `NOT IN` filter. Let the sample weights handle any remaining bad-day noise.

**4d. Update hyperparameters for larger dataset**
```python
LGBM_PARAMS = dict(
    n_estimators=1500,       # more trees for larger dataset
    learning_rate=0.04,      # slightly lower to match more trees
    num_leaves=127,
    min_child_samples=100,   # stricter (was 50) — more data means can afford higher leaf floor
    subsample=0.7,
    colsample_bytree=0.8,
    n_jobs=-1,
    verbose=-1,
    random_state=42,
)
```

---

## Implementation Order

1. [ ] `scripts/fetch_hsp_history.py` — write and run against HSP API
2. [ ] SQL cleanup — cap 28 May, verify tier distribution
3. [ ] `engine.rs` — hook record_outcome into poll loop
4. [ ] `compare_models.py` — random split + sample weighting + updated params
5. [ ] Train and compare (`python3.11 scripts/compare_models.py`)
6. [ ] Deploy new `.onnx` files, restart server
7. [ ] Re-check report metrics after 24h of live predictions

---

## Expected Outcome

| Metric | Current | Target |
|---|---|---|
| Within ±5 min | 54.6% | 65%+ |
| Within ±10 min | 70.3% | 80%+ |
| Avg prediction error | 12.3 min | 6–8 min |
| On-time MAE | 20.4 min | <8 min |

The on-time MAE improvement is the biggest signal — going from 20.4 to <8 min would mean the model stops falsely predicting delays for trains that are actually running on time.

---

## Notes

- HSP API endpoint reference: `https://hsp-prod.rockshore.net/api/v1/serviceMetrics` and `/serviceDetails`
- Auth is HTTP Basic with NROD credentials (same as `DARWIN_USERNAME` / `DARWIN_PASSWORD`)
- Rate limit conservatively at 0.5 req/s, expect a full run to take 2–6 hours
- `predicted_delay_mins` will be `NULL` for all HSP-sourced rows — this is correct, they are ground-truth history only
- After loading HSP data, re-run `make train` (not just `compare_models.py`) to rebuild the production ONNX files

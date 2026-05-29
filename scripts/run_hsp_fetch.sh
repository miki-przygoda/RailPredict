#!/usr/bin/env bash
# run_hsp_fetch.sh — Launch 4 parallel HSP fetch processes, one per NROD account.
#
# Prerequisites:
#   1. Four NROD accounts — register free at https://opendata.nationalrail.co.uk/
#      Go to your profile and tick "HSP" under Subscription Type.
#      Set credentials below (or export HSP_U1..P4 in your shell before running).
#   2. Python deps: pip install requests psycopg2-binary python-dotenv
#   3. DATABASE_URL pointing at the RailPredict postgres instance.
#
# Usage:
#   chmod +x scripts/run_hsp_fetch.sh
#   ./scripts/run_hsp_fetch.sh
#
# Progress is stored in the hsp_fetch_progress DB table — safe to Ctrl-C and rerun.

set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

# ── Credentials ─────────────────────────────────────────────────────────────
# HSP uses the National Rail Data Portal — NOT the Darwin STOMP credentials.
# Register free at https://opendata.nationalrail.co.uk/
# In your profile, tick "HSP" under Subscription Type.
# Create one account per shard (4 accounts total) for maximum parallelism.
HSP_U1="${HSP_U1:-}"
HSP_P1="${HSP_P1:-}"
HSP_U2="${HSP_U2:-}"
HSP_P2="${HSP_P2:-}"
HSP_U3="${HSP_U3:-}"
HSP_P3="${HSP_P3:-}"
HSP_U4="${HSP_U4:-}"
HSP_P4="${HSP_P4:-}"

if [[ -z "$HSP_U1" || -z "$HSP_P1" ]]; then
  echo "ERROR: set HSP_U1/P1 through HSP_U4/P4 before running."
  echo "These are National Rail Data Portal accounts, not Darwin STOMP."
  echo "Register at https://opendata.nationalrail.co.uk/"
  exit 1
fi

# ── Parameters ───────────────────────────────────────────────────────────────
DAYS="${DAYS:-90}"         # random non-holiday days per station
WORKERS="${WORKERS:-20}"   # concurrent serviceDetails threads per shard
SEED="${SEED:-42}"         # RNG seed — same seed = same day pool across all shards

echo "Starting 4 HSP fetch shards — ${DAYS} days, ${WORKERS} workers each"
echo "Progress tracked in hsp_fetch_progress DB table — safe to Ctrl-C and rerun."
echo ""

# ── Launch ────────────────────────────────────────────────────────────────────
HSP_USERNAME="$HSP_U1" HSP_PASSWORD="$HSP_P1" \
  python3 scripts/fetch_hsp_history.py \
    --shard-id 0 --shards 4 \
    --days "$DAYS" --workers "$WORKERS" --seed "$SEED" &
PID0=$!

HSP_USERNAME="$HSP_U2" HSP_PASSWORD="$HSP_P2" \
  python3 scripts/fetch_hsp_history.py \
    --shard-id 1 --shards 4 \
    --days "$DAYS" --workers "$WORKERS" --seed "$SEED" &
PID1=$!

HSP_USERNAME="$HSP_U3" HSP_PASSWORD="$HSP_P3" \
  python3 scripts/fetch_hsp_history.py \
    --shard-id 2 --shards 4 \
    --days "$DAYS" --workers "$WORKERS" --seed "$SEED" &
PID2=$!

HSP_USERNAME="$HSP_U4" HSP_PASSWORD="$HSP_P4" \
  python3 scripts/fetch_hsp_history.py \
    --shard-id 3 --shards 4 \
    --days "$DAYS" --workers "$WORKERS" --seed "$SEED" &
PID3=$!

echo "PIDs: shard0=$PID0  shard1=$PID1  shard2=$PID2  shard3=$PID3"
echo ""
echo "Watch progress:"
echo "  watch -n10 'psql \$DATABASE_URL -c \"SELECT count(*) FROM hsp_fetch_progress\"'"

wait $PID0 $PID1 $PID2 $PID3
echo ""
echo "All shards complete."

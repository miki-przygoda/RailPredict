.PHONY: up down build rebuild logs db ingest export demo map os train seed-stations seed-history

# Start all services, rebuilding the app image from current source.
up:
	docker compose up --build

# Rebuild from scratch (busts cargo-chef dep cache too — slow but guaranteed fresh).
rebuild:
	docker compose build --no-cache app
	docker compose up

# Stop and remove containers (keeps DB volume).
down:
	docker compose down

# Tail app logs.
logs:
	docker compose logs -f app

# Start only the DB (useful when running the app locally with `cargo run`).
db:
	docker compose up db

# Run the GTFS ingest job.
ingest:
	docker compose --profile ingest run --rm ingest

# Export a static HTML snapshot of the last 7 days' delay and prediction data.
# Output: docs/index.html  (an internal report — open in any browser).
# Requires a running DB with delay history — start the server first with `make up`.
# Override the window with: make export DAYS=14
DAYS ?= 7
export:
	cd RailPredict && cargo run --release -- export-site --output ../docs/index.html --days $(DAYS)

# Export the self-contained exec demo site (scrollytelling + baked replay).
# Output: docs/demo.html (an internal sales/handover artefact — open in any browser).
# Requires a running DB with delay history — start the server first with `make up`.
demo:
	cd RailPredict && cargo run --release -- export-demo --output ../docs/demo.html --days $(DAYS)


# Export the self-contained offline command-centre map (GB delay map).
# Output: docs/map.html (open in any browser — fully offline). Needs a running DB.
map:
	cd RailPredict && cargo run --release -- export-map --output ../docs/map.html --days $(DAYS)

# Export the self-contained offline "RailPredict OS" desktop. Output: docs/os.html
# (open in any browser — fully offline). Needs a running DB.
os:
	cd RailPredict && cargo run --release -- export-os --output ../docs/os.html --days $(DAYS)

# Rebuild the OS map's day-specific datasets (network, moving services, replay
# predicted-vs-actual, operator league) straight from the DB. Uses the same
# DATABASE_URL you run the app with. Defaults to the latest day present; override:
#   DATABASE_URL=… make map-data DATE=2026-06-06
#   DATABASE_URL=… make map-data FROM=2026-06-05 TO=2026-06-06
map-data:
	python3 scripts/build_map_data.py $(if $(DATE),--date $(DATE)) $(if $(FROM),--from $(FROM)) $(if $(TO),--to $(TO))

# One-shot: rebuild the datasets for DATE (or latest) and regenerate both pages.
#   DATABASE_URL=… make map-day DATE=2026-06-06
map-day:
	$(MAKE) map-data $(if $(DATE),DATE=$(DATE)) $(if $(FROM),FROM=$(FROM)) $(if $(TO),TO=$(TO))
	$(MAKE) os
	$(MAKE) map

# Backfill delay_history with 90 days of synthetic historical data.
# Uses the top TIPLOCs already seen in the live Darwin feed, so the training
# set covers exactly the routes the model will be asked to predict on.
# Safe to re-run: ON CONFLICT DO NOTHING skips any duplicate rows.
# After running: make train → restart server → open /predictions to see accuracy.
SEED_DAYS    ?= 90
SEED_STATIONS?= 120
seed-history:
	@test -d scripts/.venv || python3 -m venv scripts/.venv
	scripts/.venv/bin/pip install -q psycopg2-binary
	DATABASE_URL=$$(grep DATABASE_URL .env | cut -d= -f2- | sed 's/@db:/@localhost:/') \
	  scripts/.venv/bin/python scripts/seed_history.py \
	    --days $(SEED_DAYS) --stations $(SEED_STATIONS)

# Seed the stations table from OpenStreetMap (Overpass API — no auth required).
# Fetches every UK National Rail station with a CRS code, including TIPLOC mappings.
# Safe to re-run: uses ON CONFLICT DO UPDATE so existing rows are refreshed.
seed-stations:
	@test -d scripts/.venv || python3 -m venv scripts/.venv
	scripts/.venv/bin/pip install -q psycopg2-binary
	DATABASE_URL=$$(grep DATABASE_URL .env | cut -d= -f2- | sed 's/@db:/@localhost:/') \
	  scripts/.venv/bin/python scripts/seed_stations.py

# Train ML delay prediction models and export them as ONNX.
# Reads DATABASE_URL from .env (swaps @db: → @localhost: automatically).
# Outputs: models/day_ahead.onnx, models/realtime.onnx, models/feature_meta.json
# Restart the server after training to pick up the new models.
train:
	@test -d scripts/.venv || python3 -m venv scripts/.venv
	scripts/.venv/bin/pip install -q -r scripts/requirements.txt
	scripts/.venv/bin/python scripts/train_models.py

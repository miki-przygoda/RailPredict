.PHONY: up down build rebuild logs db ingest export train

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
# Output: docs/index.html  (open in any browser, or deploy to Vercel/GitHub Pages).
# Requires a running DB with delay history — start the server first with `make up`.
# Override the window with: make export DAYS=14
DAYS ?= 7
export:
	cd RailPredict && cargo run --release -- export-site --output ../docs/index.html --days $(DAYS)

# Train ML delay prediction models and export them as ONNX.
# Reads DATABASE_URL from .env (swaps @db: → @localhost: automatically).
# Outputs: models/day_ahead.onnx, models/realtime.onnx, models/feature_meta.json
# Restart the server after training to pick up the new models.
train:
	@test -d scripts/.venv || python3 -m venv scripts/.venv
	scripts/.venv/bin/pip install -q -r scripts/requirements.txt
	scripts/.venv/bin/python scripts/train_models.py

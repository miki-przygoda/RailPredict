.PHONY: up down build rebuild logs db ingest

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

# syntax=docker/dockerfile:1

# ─── Stage 1: Planner ─────────────────────────────────────────────────────────
# cargo-chef analyses the workspace and emits a recipe.json describing only the
# dependency graph. As long as Cargo.toml / Cargo.lock don't change, every
# subsequent stage that reads recipe.json gets a cache hit.
FROM rust:1.82-bookworm AS planner
WORKDIR /app
RUN cargo install cargo-chef --locked
COPY RailPredict/Cargo.toml RailPredict/Cargo.lock ./
COPY RailPredict/src ./src
RUN cargo chef prepare --recipe-path recipe.json

# ─── Stage 2: Cacher ──────────────────────────────────────────────────────────
# Builds all dependencies (the slow part). Cached as a layer until recipe.json
# changes (i.e. until Cargo.lock changes).
FROM rust:1.82-bookworm AS cacher
WORKDIR /app
RUN cargo install cargo-chef --locked
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# ─── Stage 3: Builder ─────────────────────────────────────────────────────────
# Builds only the application code. Dependency compilation is already done —
# this stage compiles just src/ on top of the cached deps.
FROM rust:1.82-bookworm AS builder
WORKDIR /app

# sqlx offline mode: compile-time query checking without a live DB.
# Run `cargo sqlx prepare` locally and commit the generated .sqlx/ directory.
ENV SQLX_OFFLINE=true

COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
COPY RailPredict/Cargo.toml RailPredict/Cargo.lock ./
COPY RailPredict/src ./src
# Migrations are embedded by sqlx::migrate! at compile time.
COPY migrations ./migrations

RUN cargo build --release --bin railpredict

# ─── Stage 4: Runtime ─────────────────────────────────────────────────────────
# Minimal Debian image. glibc is required by reqwest (rustls-tls still links
# against system libssl on bookworm). Alpine/musl requires a cross-compilation
# toolchain change with no meaningful size benefit at this scale.
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
        curl \
    && rm -rf /var/lib/apt/lists/*

# Run as a non-root user.
RUN useradd --create-home --shell /bin/bash railpredict
USER railpredict
WORKDIR /home/railpredict

COPY --from=builder /app/target/release/railpredict /usr/local/bin/railpredict

EXPOSE 3000

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

ENTRYPOINT ["/usr/local/bin/railpredict"]

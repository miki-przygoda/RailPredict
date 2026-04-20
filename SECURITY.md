# Security — Secrets and Rotation

This document lists every secret used by RailPredict, how to rotate each one, and the
recommended rotation cadence.

---

## Secrets inventory

| Secret          | Env var           | Where it is used                                          |
|-----------------|-------------------|-----------------------------------------------------------|
| GBR API key     | `GBR_API_KEY`     | `x-apikey` header on all outbound GBR Retail API calls    |
| Darwin password | `DARWIN_PASSWORD` | STOMP CONNECT frame to the Darwin Push Port broker        |
| DB password     | `DB_PASSWORD`     | Part of `DATABASE_URL`; authenticates the PostgreSQL user |

---

## How to rotate each secret

### GBR_API_KEY

1. Log in to the RTT / GBR developer portal and generate a new API key.
2. Update the `GBR_API_KEY` value in your deployment environment (`.env` file,
   secrets manager, or CI/CD secret store).
3. Re-deploy the application. The new key is picked up at startup via `Config::from_env`.
   No live reload or cache flush is needed — the key is used per-request.
4. Verify the new key is working by checking the `gbr_api_requests_total` Prometheus
   metric and confirming there are no `401` error responses.
5. Revoke the old key in the developer portal once the new deployment is confirmed healthy.

### DARWIN_PASSWORD

1. Request a new password from the Network Rail Open Data portal
   (https://opendata.nationalrail.co.uk/).
2. Update `DARWIN_PASSWORD` (and `DARWIN_USERNAME` if changed) in your deployment
   environment.
3. Re-deploy. The new credentials are used on the next STOMP CONNECT attempt.
   The auto-reconnect loop in `main.rs` will pick up the new credentials automatically
   on the next reconnect cycle — no manual restart required after the initial deploy.
4. Confirm the Darwin stream is active by checking `darwin_messages_received_total` in
   Prometheus. The counter should resume incrementing within 30–60 seconds.

### DB_PASSWORD

1. Generate a new strong password (at least 32 random characters).
2. In PostgreSQL, run:
   ```sql
   ALTER ROLE railpredict PASSWORD 'new-password-here';
   ```
3. Update `DB_PASSWORD` and the `DATABASE_URL` (which embeds the password) in your
   deployment environment simultaneously.
4. Re-deploy. sqlx opens a fresh connection pool at startup with the new credentials.
5. Verify the application connects successfully by checking the health endpoint:
   `GET /health` should return `{ "status": "ok" }`.

---

## Recommended rotation cadence

| Secret            | Cadence                                                      |
|-------------------|--------------------------------------------------------------|
| `GBR_API_KEY`     | On personnel change, or immediately on suspected exposure    |
| `DARWIN_PASSWORD` | On personnel change, or immediately on suspected exposure    |
| `DB_PASSWORD`     | Quarterly minimum; immediately on personnel change or breach |

---

## Additional security notes

- **CORS**: In production, `CORS_ALLOWED_ORIGINS` must be set to a comma-separated list
  of allowed origins. The application will refuse to start in non-debug mode without it.
  See `.env.example` for the format.

- **Rate limiting**: The public API is rate-limited to `HTTP_RATE_LIMIT_PER_SEC` requests
  per second per IP (default: 60). The `/health` and `/metrics` endpoints are excluded.

- **TLS**: Darwin Push Port connections use TLS by default (`DARWIN_TLS=true`). Set
  `DARWIN_TLS=false` only for local mock brokers in development.

- **Never commit `.env`**: The `.env` file is listed in `.gitignore`. Only `.env.example`
  (with empty or example values) is committed to the repository.

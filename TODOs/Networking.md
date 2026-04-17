# TODOs/Networking.md – The External Handshake

This is where the "GBR Bottleneck" is managed.

### Key considerations for Networking:

1. **Request Coalescing (The "Waiter" Pattern)**:
    - If 10 users ask for the same train status, implement a mechanism where only 1 outgoing request is made, and the result is "fanned out" to all 10 waiting `oneshot` channels.

2. **Backpressure & Rate Limiting**:
    - GBR APIs will block you if you are too aggressive.
    - Build a `RateLimiter` middleware that queues outgoing requests if you hit a pre-defined threshold.

3. **Circuit Breakers**:
    - If the GBR API returns a 503 (Overloaded), your system should automatically switch to "Cache Only" mode and stop sending requests for a "cool-down" period.

---

### Before Starting: Pre-work Subtasks

These must be done before writing any implementation code for this epic. Epic 1 (Structs) must be complete. Epic 5 (Cache stub) must exist as a writable target.

- [ ] **Apply for GBR / Darwin API credentials NOW** — this is time-gated and outside your control. Register at the National Rail open data portal for Darwin Push Port access and separately for the GBR Retail API if applicable. These can take days. Do this before any other pre-work for this epic. Note the credential types (API key, OAuth, basic auth) when they arrive — the `gbr_client.rs` auth layer depends on this.

- [ ] **Cargo.toml: add networking dependencies** — `reqwest` with `features = ["json", "rustls-tls"]` (prefer rustls over native-tls for portability), `tower` or a manual implementation for the rate limiter middleware, `async-trait` if you define a `GbrClient` trait for testability.

- [ ] **Document the specific GBR endpoints before writing the client** — list the exact URLs, HTTP methods, auth headers, and expected response shapes for every endpoint this client will call. Store these as constants or a config struct in `gbr_client.rs`. Never hardcode a URL in a function body.

- [ ] **Understand `tokio::sync::oneshot` vs `tokio::sync::broadcast` before writing the coalescer** — `oneshot` is for one sender, one receiver (a single user waiting on a single pending request). For the coalescer you need a `Vec<oneshot::Sender<T>>` attached to each in-flight request key. `broadcast` is for the state machine's notification channel (one sender, many receivers). Know which is which before writing a line of the coalescer.

- [ ] **Define the rate limit constants explicitly** — what is GBR's published rate limit? If not published, what is a safe conservative value? Define these as named constants (e.g., `MAX_REQUESTS_PER_SECOND: u32 = 10`) with a comment explaining the source of the value. The rate limiter implementation depends on these being fixed before it is written.

- [ ] **Design the `CircuitBreakerState` type before the circuit breaker logic** — the breaker needs at minimum: `Closed` (normal), `Open` (blocking all requests), `HalfOpen` (testing if GBR has recovered). This is its own small state machine. Sketch the transitions (what triggers Open, what timer triggers HalfOpen, what success criteria close it again) before writing the struct.

- [ ] **Create the module skeleton** — create `src/networking/mod.rs`, `src/networking/gbr_client.rs`, `src/networking/coalescer.rs`, `src/networking/rate_limiter.rs`, `src/networking/circuit_breaker.rs` as stubs. Compile before adding logic.

- [ ] **Note: the networking layer must be testable without live credentials** — design `GbrClient` as a trait from the start so tests can inject a mock. If you write it as a concrete struct with hardcoded HTTP calls, you will not be able to unit test the coalescer, rate limiter, or circuit breaker logic without live GBR access.

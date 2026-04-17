# TODOs/StateMachine.md – The Logic Engine

The State Machine dictates the "Pulse" of your system. It decides when to be aggressive and when to be lazy.

### Key considerations for the State Machine:

1. **State Transitions**:
    - Define an `enum TrainState { Dormant, Monitored, Active, Critical }`.
    - Consider what triggers a "Promotion" (e.g., time getting closer) vs. an "Emergency Promotion" (e.g., a signal failure reported in the firehose).

2. **The Polling Loop**:
    - Should each train have its own `tokio::spawn` task, or should one global manager loop through all active trains?
    - *Recommendation*: Use a global manager with a priority queue (`BinaryHeap`) to handle the next scheduled poll time.

3. **Concurrency Safety**:
    - If a state changes (e.g., from `Monitored` to `Active`), how do you notify the UI/API layer without locking the entire database?
    - Consider an `mpsc` channel to broadcast state changes to a "Notification Service."

---

### Before Starting: Pre-work Subtasks

These must be done before writing any implementation code for this epic. Epic 1 (Structs) must be complete first.

- [ ] **Cargo.toml: add async runtime dependencies** — `tokio` with `features = ["full"]`. Confirm `tokio::time`, `tokio::sync`, and `tokio::sync::mpsc` are available. Add `tokio-util` if you need stream utilities for the poll manager.

- [ ] **Draw the complete state transition diagram before writing a single `match` arm** — map every possible edge, not just the happy path. Specifically: does a train demote from `Active` back to `Monitored` if it gets significantly delayed (departure pushed back past the 30-min threshold)? What is the terminal state when a train has departed or been cancelled? What happens if a `Critical` train's volatility event resolves? Write these answers as doc comments on the `TrainState` enum before implementing transitions.

- [ ] **Define the `BinaryHeap` entry type explicitly** — the heap needs to store `(next_poll_at: Instant, train_id: TrainID)`. This entry type must implement `Ord` ordered by `next_poll_at` in reverse (min-heap semantics). Define this type in `src/state_machine/poll_manager.rs` as a named struct before building the manager loop.

- [ ] **Understand `tokio::select!` before building the poll loop** — the manager loop cannot simply `sleep_until(next_poll_time)` because new trains can be added to the heap mid-sleep. You need `tokio::select!` to race between the sleep and a "new train registered" notification channel. Read the `tokio::select!` docs and write a small standalone example before integrating it.

- [ ] **Decide `mpsc` channel buffer sizes** — the notification channel that broadcasts state changes to the API layer needs a bounded buffer. Too small and you block the state machine under load; too large and memory grows unbounded on a slow consumer. Decide the bound before the channel is created and document the reasoning as a constant in the code.

- [ ] **Create the module skeleton** — create `src/state_machine/mod.rs`, `src/state_machine/train_state.rs`, `src/state_machine/poll_manager.rs` as stubs. Wire them into `main.rs`. Compile on the empty skeleton before adding logic.

- [ ] **Note: emergency promotions depend on ingestion** — the volatility-triggered state changes (weather, incident detection) can only be fully implemented once the ingestion pipeline (Epic 4) is in place. Design the `TrainState` transition logic to accept a `PromotionReason` enum from the start, but leave the ingestion-triggered arm as `todo!()` until Epic 4 is done.

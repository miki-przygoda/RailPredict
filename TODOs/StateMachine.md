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
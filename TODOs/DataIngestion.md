# TODOs/DataIngestion.md – The Firehose (Darwin)

Handling the STOMP firehose from GBR is like drinking from a firehose.

### Key considerations for Ingestion:

1. **The Firehose Filter**:
    - You don't need *every* update for *every* train in the UK.
    - Implement a "Region or Route Filter" early in the ingestion pipeline to drop packets that aren't relevant to your current user base.

2. **Sequence Numbering**:
    - Messages can arrive out of order.
    - Your ingestion logic must check the `timestamp` or `sequence_id` of the update. Never overwrite a newer update with an older one that arrived late.

3. **Message Parsing Latency**:
    - Use `serde-xml-rs` or a high-speed XML parser. Darwin data is notoriously verbose XML. Optimization here is critical for the "Instant" feel.

---

### Before Starting: Pre-work Subtasks

These must be done before writing any implementation code for this epic. Epic 1 (Structs) must be complete. Epic 5 (Cache stub) must exist as a writable target.

- [ ] **Apply for Darwin Push Port access through Network Rail's open data portal** — this is separate from the GBR API credentials in Networking.md and uses a different registration process. The Push Port uses STOMP over a message broker (typically ActiveMQ). Credentials include a hostname, port, username, and password. Start the application now as approval can take time.

- [ ] **Download real Darwin sample XML messages before writing any parser** — Network Rail's developer documentation includes example Push Port messages. Download samples of every message type you plan to handle: `TS` (Train Status), `PP` (Push Port), `SF` (Schedule Forecast), `deactivated`. You cannot write a correct parser against documentation alone — the actual XML has edge cases the docs don't mention.

- [ ] **Cargo.toml: add ingestion dependencies** — `serde-xml-rs` or `quick-xml` (quick-xml is faster and more actively maintained; benchmark both against a real Darwin payload before choosing). Add a STOMP client library — research the current state of Rust STOMP crates; `stomp-rs` has been dormant, consider `async-stomp` or writing a thin wrapper over a raw TCP connection with `tokio::net::TcpStream`. Document your choice here.

- [ ] **Map the Darwin message type taxonomy before writing the filter** — the firehose carries many message types. List every type and mark each as: `KEEP` (relevant to train status), `CONDITIONAL` (relevant only for specific routes/trains), or `DROP` (never needed). The filter implementation is a match on this taxonomy, so defining it first makes the code trivial to write.

- [ ] **Understand the Darwin `sequence_id` semantics** — Darwin's out-of-order delivery is not just a network issue; the Push Port can replay messages on reconnect. Understand whether `sequence_id` is per-train or global, and whether it resets on reconnect. This determines whether your sequence check is a per-`TrainID` counter or a global one, and what happens when the STOMP connection drops and reconnects.

- [ ] **Decide on the ingestion channel buffer size** — between the STOMP receiver and the filter/parser workers, there is a bounded async channel. The Darwin firehose at peak can deliver hundreds of messages per second. If the filter or parser is slow, this buffer fills and the STOMP consumer blocks. Set the buffer size as a named constant with documented reasoning, not an arbitrary magic number.

- [ ] **Create the module skeleton** — create `src/ingestion/mod.rs`, `src/ingestion/stomp_client.rs`, `src/ingestion/filter.rs`, `src/ingestion/parser.rs` as stubs. Compile before adding logic.

- [ ] **Note: emergency promotion hook into the state machine** — when the parser detects a disruption message (e.g., a cancelled train, a major delay), it must trigger an emergency state promotion in the state machine. This coupling between `ingestion/` and `state_machine/` should go through the `mpsc` notification channel defined in Epic 2, not a direct function call. Do not create a direct dependency from the ingestion module into the state machine module.

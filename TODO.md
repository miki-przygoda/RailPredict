## TODO and TODOs:
- This is the main TODO file for the project. This file will change with the most recent things that need to be worked on. Once all changes are completed for a specific todo set, the version of the project should be changed. Per TODO point by one so (from x.x.1 to x.x.2) and when a full section is complete (from x.1.x to x.2.0).

The current version and last worked on date should be noted at the top of this file below this line:

**version = "0.1.0" -- 17/04/2026**

- More specialised TODOs are found in `TODOs/` and should be created and destroyed as needed for specific larger tasks. Before the creation and destruction of each TODO file, (if created note the epic in here and link to the specific TODO if needed, if destroyed note the completion of the epic into the `CHANGELOG.md` and update the version both in the `TODO.md`, `CHANGELOG.md`, `README.md`, `CLAUDE.md` and `Cargo.toml`).
- The version should be seen as sacred -- after a larger completion all parts of the project should have the exact same version format, dates are used to easily track git changes. Most importantly, once all versions match and the markdown files are updated you should move on with the further parts of the TODO; This section up here should not be changed under any circumstances;

### Strategic Starting Points:

- The Identifier Problem (Structs.md): In Rust, I recommend creating a custom `TrainID` enum or struct that can wrap `RID`, `UID`, and `Headcode`. This makes it much easier to write lookups that work regardless of which "ID" the GBR API decides to send you in a specific message.

- The Waiter Pattern (Networking.md): This is the secret to low latency. Instead of making a network call for every user request, you use `tokio::sync::oneshot` channels to have multiple users "subscribe" to a single pending API call.

- Backpressure (Networking.md): GBR is notoriously protective of their servers. If you don't build a circuit breaker now, you'll find your API keys revoked the first time you get a spike in traffic.

- The Ingestion Filter (DataIngestion.md): The Darwin push port sends everything. If you don't filter it immediately, your CPU will spend 90% of its time parsing XML for trains that none of your users are looking at.


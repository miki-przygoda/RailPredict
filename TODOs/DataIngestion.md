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
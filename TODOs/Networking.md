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
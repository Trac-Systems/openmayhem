# OpenMayhem 0.2.225

- Pins HyperDHT 6.29.6 across the bundled Intercom, settlement-bus, and peer runtime to prevent long-running nodes from crashing when persistent DHT handlers receive traffic while the node is ephemeral.
- Rejects release packages that contain a stale or duplicate HyperDHT or settlement-bus dependency tree.

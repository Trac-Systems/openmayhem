# OpenMayhem Core 0.2.230

- TNK deposit readers now catch up to the canonical peer's current MSB signed height before scanning incoming transfers.
- The TNK watcher allows a bounded five-minute reader catch-up window, preventing a retained sparse reader store from repeatedly scanning an obsolete frontier.
- Contract version and contract code digest remain unchanged.

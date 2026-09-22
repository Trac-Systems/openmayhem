# OpenMayhem 0.2.259

- Retire confirmed partial receipt deliveries independently of final settlement, allowing failed-session reservation recovery to progress while preserving signed usage evidence.
- Avoid repeatedly validating unrelated pending receipts on every streaming checkpoint. Keep cross-process locking, durable writes, monotonic receipt validation, and full background validation.
- Allow existing receipt attempts to advance when the outbox reaches its capacity limit.

The contract version, model catalog, calibration requirements, and receipt storage layout are unchanged.

# Retained contract implementations

These files execute authenticated historical operations using their original
version rules. The current contract remains version 30. New submissions must
still use the current version; an old version number alone never grants replay.

`origins-17-22.json` records the original source commits and SHA-256 hashes for
the implementations restored for fresh synchronization. Their bytes are unchanged
and included in the release identity. Retained versions 23–29 continue through
the same authenticated replay path. Receipt recovery has its own, unchanged
version policy.

Validation covers exact state and tree hashes for persisted signed Feature and
paid-Tx replay, real remote Autobase catch-up, duplicate suppression and rejection
of new, modified, optimistic or unbound historical operations. A fresh mainnet
reader also catches up through authenticated fast-forward and is compared with
the live writer. This does not claim that every historical operation can be
re-executed from genesis with a single source revision for each version.

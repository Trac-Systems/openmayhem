# 0.2.197

Fixes historical contract replay on peers rebuilding an older signed view. Replay
acceptance evidence now comes from an owned read-only session on the authenticated
default view core, so it remains available while the peer's apply batch is still
materializing an earlier prefix.

The existing replay safeguards remain in place: the original signed input bytes,
matching signed acceptance records, exact operation identity, and one-use execution
capability are still required. Contract version remains 25 and this patch requires
no additional pricing migration.

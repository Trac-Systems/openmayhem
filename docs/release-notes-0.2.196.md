# 0.2.196

Fixes historical contract v23/v24 replay after a contract v25 upgrade. Authenticated
feature and paid transaction history now uses retained contract implementations,
so rebuilding an older view preserves its original state and pricing semantics.
New older-version submissions remain rejected. Replay requires the original signed
input and matching acceptance records in the signed canonical view.

The release identity binds the retained implementations and replay admission code.
Contract version remains 25; this patch has a new code digest and needs no additional
pricing migration. Existing prepared checkpoint and signed receipt recovery remain
supported.

The read-only status endpoint can return an applied-view prefix hash. Upgrade checks
can compare materialized state against a canonical boundary and observe replay
completion instead of relying only on early health or sparse signed lengths.

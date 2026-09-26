# OpenMayhem Core 0.2.233

This release restores receipt recovery across supported contract versions and
prevents obsolete checkpoint evidence from blocking current inference. A
confirmed canonical receipt head can now safely retire a superseded non-final
checkpoint when its attempt, terms, usage, and sequence prove continuity.

The Intercom contract remains version 25 with unchanged contract bytes.

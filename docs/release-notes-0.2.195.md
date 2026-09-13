# 0.2.195

Fixes paid checkpoint recovery after upgrading from contract v24 to v25. A durably
prepared checkpoint can finish using its original signed payment when it matches
the canonical preparation. Retries apply once and do not pay a second fee.

Fresh legacy operations remain rejected. Recovery verifies both canonical snapshot
hashes and retains the exact signed dispatch revision. Original signed v23/v24
receipt recovery remains supported. Contract version remains 25; the release has a
new verified code digest.

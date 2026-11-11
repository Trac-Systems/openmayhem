# 0.2.206

Calibration memory measurement now tolerates managed descendant processes that
exit between in-flight sampling and the final RSS sample. It still requires a
live measured process and a successful in-flight sample, and operation errors
remain authoritative.

Contract version remains 25 and this release requires no pricing migration.

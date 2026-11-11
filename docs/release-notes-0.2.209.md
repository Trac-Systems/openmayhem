# 0.2.209

OpenAI-compatible prefix-cache preflight now supports Prometheus counters that
are created lazily on the first cache hit. The identical-prefix replay must
still expose the signed counter and prove that its value increased.

Contract version remains 25 and this release requires no pricing migration.

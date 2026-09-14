# 0.2.215

Direct peer channels now remove transport state on every terminal event and
recover a half-open connection only after the existing bidirectional health
protocol proves it dead. Healthy transports and unrelated peers are preserved,
and relay failures retain an allowlisted delivery phase for diagnosis.

Contract version remains 25 and this release requires no pricing migration.

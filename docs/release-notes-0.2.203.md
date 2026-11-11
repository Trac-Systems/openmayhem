# 0.2.203

OpenAI-compatible concurrency preflight now polls the signed scheduler metric
while its bounded request workers are live. Admission requires both observed
scheduler overlap at the signed concurrency and first streamed content from
every worker before Core cancels the probes.

Contract version remains 25 and this release requires no pricing migration.

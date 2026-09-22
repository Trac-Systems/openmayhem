# 0.2.202

OpenAI-compatible runtime preflight now gives every signed chat-template
control a unique parameter identity derived from its native path. Profiles that
carry several reasoning controls pass request validation while preserving each
exact signed control value.

Contract version remains 25 and this release requires no pricing migration.

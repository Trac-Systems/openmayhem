# 0.2.210

Endpoint calibration now extracts base64 video fixtures from standard
`data:video/*;base64,...` URLs, including nested OpenAI-compatible
`video_url` inputs. This makes video-capable endpoint contracts testable
without model-specific fixture handling.

Contract version remains 25 and this release requires no pricing migration.

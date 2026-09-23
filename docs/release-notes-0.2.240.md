# OpenMayhem Core 0.2.240

Large client output limits are treated as upper bounds and reduced to the
context that remains after each accumulated prompt. Streaming and non-streaming
chat therefore keep using eligible providers as conversations grow instead of
failing route selection when the requested output allowance alone fills the
model context window. Context failures are reported directly without a route
wait or provider cooldown.

Authenticated clients can obtain the serving runtime's exact, template-aware
token count through `POST /v1/tokenize` or `POST /v1/count_tokens`. The control
request accepts chat `messages` or a single `prompt`, can optionally return token
IDs, and creates no inference charge, receipt, or capacity reservation. vLLM
providers answer token-count requests while generations are active instead of
holding them behind the generation queue. A runtime without an exact tokenizer
returns `token_count_unsupported` instead of an estimate.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged; no recalibration is required.

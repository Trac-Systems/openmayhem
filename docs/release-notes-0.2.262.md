# OpenMayhem 0.2.262

OpenAI-compatible providers no longer report success when a complete tool call is emitted only inside private reasoning. When the call names an advertised tool, the response has no answer or structured call, and a signed non-thinking profile and output-token budget are available, the provider makes one bounded recovery attempt. It preserves the caller's tool choice and remaining output-token budget. If recovery cannot produce a structured call or answer, the request fails instead of silently ending without the requested action.

Ordinary chat, reasoning, and structured tool calls keep their existing path. No contract code, receipt schema, catalog model entry, or stored ledger format changes in this release.

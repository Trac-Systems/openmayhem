# OpenMayhem 0.2.262

OpenAI-compatible providers no longer report success when a complete tool call is emitted only inside private reasoning. When the call names an advertised tool, the response has no answer or structured call, and a signed non-thinking profile and output-token budget are available, the provider makes one bounded recovery attempt. It preserves the caller's tool choice and remaining output-token budget. If recovery cannot produce a structured call or answer, the request fails instead of silently ending without the requested action.

Ordinary chat, reasoning, and structured tool calls keep their existing path. No contract code, receipt schema, or stored ledger format changes in this release.

Provider admission now reads only the confirmed records for the selected route instead of scanning every historical price version. This removes catalog-history growth from the per-request admission path while retaining confirmed ledger checks.

Needle's CPU and GPU runtimes can serve their fixed-context chat endpoint without claiming prefix caching. The prefix-cache admission requirement remains in force for other text-generation runtimes.

Laya's signed catalog reference rate is reduced; its live market schedule is updated separately through the existing admin pricing process.

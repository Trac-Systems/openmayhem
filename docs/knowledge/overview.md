---
type: Reference
title: "What OpenMayhem Is"
description: "The one-liner, the full definition, the thesis behind the project, and how it positions against OpenRouter and DePIN compute networks."
tags: [overview, vision, thesis, positioning]
generated: { by: "human:muffin", at: "2026-07-21T00:00:00Z"}
verified:
  - { by: "process:wiki-refresh", at: "2026-08-02T00:00:00+02:00"}
  - { by: "process:source-and-catalog-sweep", at: "2026-09-02T12:00:00+02:00"}
status: stable
stale_after: 2026-10-02
---

# What OpenMayhem Is

## One-paragraph orientation
OpenMayhem is a peer-to-peer AI inference marketplace on Trac Network. Users point any
OpenAI-compatible client at a local gateway (`127.0.0.1:11435`) and buy inference directly from
provider machines over encrypted P2P; an admin-only replicated contract on an Intercom subnet holds
the catalog, prices, balances, and settlement. Everything is priced in dollars (`au_usd`, atto-USD).
Nobody sets the running price — the admin seeds each per-enclave market once, then a
activity-momentum controller floats it. Four attestation tiers (software, TPM device identity,
confidential compute, KYB business), each its own priced market, back trust with evidence: signed
receipts, canary probes, holdbacks, and permissionless fraud proofs. Three isolated payment rails
(Stripe, Ethereum/TAP, Trac/TNK) carry value but never mix; providers keep 85% on fiat/TNK and
75% on TAP after the on-chain burn. It launched to mainnet on 2026-07-11 with a qwen3.6-35B launch
model and is onboarding a model roster one at a time.

## One line
"Sell inference from any machine — a gaming PC or a confidential-compute rack. Buy inference at
a price no company sets." (README.md)

## Full definition
OpenMayhem (aka Mayhem; CLI binary `mayhem`) is a **peer-to-peer AI inference marketplace built on
Trac Network**. Providers plug in machines — a gaming PC that sits idle overnight, a Mac, a homelab
box, or a rack of confidential-compute H100s — and earn on every token they serve. Users point any
OpenAI-compatible client at `127.0.0.1` and buy at whatever the market currently charges, paying by
card or on-chain. There is no cloud in the middle: requests travel over encrypted peer-to-peer
sessions directly from the user's local gateway to a provider's machine, and a public ledger
records prices, receipts, and settlements so anyone can verify what happened.

Internal canonical definition (`docs/CONCEPTS.md` §1): "Mayhem is a P2P AI-inference marketplace on
Trac. There is no cloud between user and model: a user's gateway buys inference directly from
provider machines over an encrypted P2P transport, and a contract on an Intercom subnet settles the
money."

Original framing (`docs/PLAN-2026-07-02`): "a peer-to-peer OpenRouter." Closing line of the README:
"The machines are already bought. The models are already open. OpenMayhem is the market that
connects them."

Identity: repo `Trac-Systems/openmayhem`, MIT license, © Trac Systems UG (haftungsbeschränkt).
Website openmayhem.ai, community r/Open_Mayhem, models at huggingface.co/TracNetwork (every served
model mirrored, signed catalog published from there).

The current source catalog also includes provider-served ComfyUI workflow markets. Users submit
policy-bounded graphs built from signed parts and approved nodes through `/v1/workflows`; providers
prove the exact runtime, parts inventory, graph envelope, and measured memory admission before a
route can advertise. Async workflow jobs and artifacts survive client disconnects and settle through
the same voucher, receipt, epoch, and payout machinery as native model endpoints.

## The thesis (why it exists)
From `docs/articles/openmayhem-medium.md` — three converging facts:

1. **Idle consumer hardware is the biggest compute pool on Earth.** In 2024 the world bought ~251
   million GPUs; Nvidia shipped ~2 million H100s to data centers in the same period. Most of that
   consumer compute is idle most of the time. "The largest computer on Earth is idle right now, and
   it belongs to ordinary people."
2. **Inference demand is compounding faster than data centers can be built.** OpenRouter grew from
   5 to 25 trillion tokens/week in six months.
3. **The agentic shift multiplies tokens.** A single agent task can consume 100–1000× the tokens of
   a chat message. Usage is shifting from conversations to workloads.

"OpenMayhem exists at the intersection of those three facts." Supporting sub-theses: agent loops
run mostly on small 3B–14B models that already run well on consumer hardware ("the small models are
the workhorse of this network"); each open-weights generation lands roughly where the closed
frontier stood a year earlier, so consumer hardware will do today's frontier work within ~12 months
while "the hardware stays, the intelligence upgrades for free"; and the direct-income consequence —
"own a piece of the machine … income from AI, paid by AI's own growth, to the people who plug in."

The reframe: "When the industry says 'AI infrastructure,' it means someone else's building. Here it
means your desk, my desk, and a few million others, wired together." The missing piece was never the
models or the hardware — it was "a market that matches each step to the cheapest machine that clears
your quality and assurance bar."

## How it differs from OpenRouter and DePIN compute
- **No pricing meeting.** Every retail AI provider has someone decide what a million tokens costs.
  OpenMayhem replaces that with an automated market maker (see [The Utilization-Indexed Pricing Controller](/market/pricing-controller.md)).
  Same OpenAI-compatible UX and many-models-one-balance surface as OpenRouter, without the central
  inference infrastructure or company margin.
- **No cloud in the middle.** Encrypted P2P direct to the provider; tokens never pass through the
  admin. Relays, when needed, forward ciphertext they cannot read, alter, or replay.
- **A payment option, not a crypto project.** Everything is priced in dollars, network gas is
  sponsored, and you never need to hold a token to use or provide. No staking anywhere — security
  comes from earnings holdback, not bonds. See [Payments, rails, and settlement](/payments/index.md).
- **Verifiability as the product.** Prices ship with recomputable derivations; receipts are signed;
  settlement is public per epoch; anyone can file a fraud proof; paid canary probes indistinguishable
  from real traffic confirm the claimed weights are served. "You are not asked to believe a number
  on a screen."
- **Trust as priced markets.** Four attestation tiers, each its own market with its own price,
  including TPM hardware identity (Tier 2), confidential compute (Tier 3), and KYB'd business
  identity (Tier 4). See [The Four Attestation Tiers](/trust/attestation-tiers.md).
- **The whole road, not just text.** Chat, tools, JSON, streaming, vision, embeddings, image
  generation, TTS/ASR, music, video routes across NVIDIA / Apple Silicon / AMD / CPU-only, engines
  chosen automatically. "Words are the busiest lane today, but the network was built for the whole
  road."

See [Design Principles and Standing Laws](/principles.md) for the design laws, the
[Glossary](/glossary.md) for terms, and [the knowledge-base index](/index.md) for the full map.

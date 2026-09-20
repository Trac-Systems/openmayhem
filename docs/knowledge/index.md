---
okf_version: "0.2"
---

# OpenMayhem Knowledge Base

An [OKF](https://github.com/GoogleCloudPlatform/knowledge-catalog) (Open Knowledge Format v0.2)
bundle. One place to hold the whole picture of OpenMayhem — the P2P AI inference marketplace — because
the knowledge is otherwise scattered across code, `docs/`, `.mayhem-local/`, and dozens of handoffs.
This is meant to grow: add a concept document when something new is learned, and log it in
[log.md](log.md).

Current source snapshot (2026-09-03): release `v0.2.164`, contract 21, receipt schema 11, and 22
signed catalog rows including 11 Comfy workflow classes. This is a repository snapshot, not a claim
that every fleet process or catalog row is live; use fresh process inventory and heartbeats for
operational state.

**Read [00-rules-and-credentials.md](00-rules-and-credentials.md) first** — the standing rules, the
custody model, and every credential.

## Start here
* [Rules, custody, and credentials](/00-rules-and-credentials.md) - the standing rules and all passwords/keys. READ FIRST.
* [What OpenMayhem is](/overview.md) - the one-liner, the full definition, the thesis, and how it differs from OpenRouter and DePIN.
* [Design principles and standing laws](/principles.md) - the core functionality that must never be removed, and the NO-STUBS mandate.
* [Glossary](/glossary.md) - every term: epoch, enclave, room, rail, au, market, receipts, holdback, canary.
* [OKF maintenance rules](/okf-maintenance.md) - how to keep this bundle compliant with Google OKF v0.2 and avoid local wiki drift.

## Market and money
* [Market and pricing](/market/index.md) - the utilization-indexed clearing price, reservation bands, epochs and provenance.
* [Payments, rails, and settlement](/payments/index.md) - the three rails, payouts, epoch settlement, and fraud proofs.

## How it works
* [Technical architecture](/architecture/index.md) - control vs data plane, the contract, the gateway, serving, and the P2P transport.
* [Trust and attestation](/trust/index.md) - the four tiers, TPM2, and confidential compute.

## Running it
* [Operations](/operations/index.md) - install and run, the CLI, provider setup, calibration, the model roster, epoch/settlement ops, release and CI, configuration, the VPN split-tunnel.
* [Infrastructure](/infrastructure/index.md) - the machines, the admin box, the VPN hub, the mainnet systemd stack.
* [Security](/security/index.md) - the audits, findings, and posture.

## Where it's going
* [Roadmap](/roadmap/index.md) - the v2 decentralization track and the planned Creator Workflows product line.
* [History](/history/index.md) - the four iterations, the plans, and the handoff index.

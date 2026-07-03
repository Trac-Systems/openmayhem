# Mayhem Router Rules v1

These rules are the consent target for Mayhem router participants. The active
contract record stores a version number and the BLAKE3 hash of this file. A
participant consents by signing `mayhem-consent<version><hash>` with the wallet
that will use the network.

Mayhem is a public, decentralized evidence ledger for a curated peer-to-peer AI
router. The ledger records consent, the admin-created enclave catalog,
canonical rooms, prices, parameter schedules, balances, evidence roots, disputes,
and provider status. Ephemeral Intercom sidechannels carry discovery and session
traffic, but they do not define canonical economic or catalog terms.

## 1. Roles And Authority

- The network admin controls the canonical economy and control plane at launch.
  The admin creates and signs enclave catalog entries, opens canonical rooms,
  sets prices, sets params, sets these rules, and may ban providers.
- Providers may register themselves, join or leave existing admin-created
  enclaves, and join or leave existing admin-created rooms. Providers may not
  submit arbitrary models, arbitrary enclaves, canonical rooms, prices, params,
  or alternate economic terms.
- Users may use canonical rooms and enclaves under the prices and policies
  recorded in the contract.
- Auditors may test service quality and evidence consistency only through the
  auditor process defined by the contract and these rules. Provider keys may
  not act as auditor keys.
- Any Intercom room, sidechannel, price, model, or offer that is not backed by
  canonical contract state is informational only and must be ignored by routers,
  users, and settlement logic.

## 2. Acceptable Use

Participants must not use Mayhem to request, provide, or coordinate content or
activity that is illegal, abusive, privacy-invasive, or intended to cause harm.
This includes instructions for credential theft, malware, fraud, targeted
harassment, sexual exploitation, non-consensual intimate content, and attempts
to bypass safety or metering controls. Admin moderation may remove access,
freeze earning eligibility, or open a dispute for suspected abuse.

Providers may refuse a session that appears to violate these rules. Users remain
responsible for their prompts, tool calls, uploaded content, and downstream use
of model output.

## 3. Provider Service Expectations

Providers must serve only the admin-created enclave catalog entries they have
joined on-chain. They must run the attested enclave build, model artifact,
manifest, and binary hash recorded for that enclave. They must not substitute
weights, alter the runtime, disable attestation checks, impersonate another
provider, or advertise capacity they do not intend to provide.

Heartbeats, saturation reports, attestation summaries, room offers, and session
acceptance messages must be honest at the time they are sent. Providers should
leave a room or enclave when they cannot serve it. Providers must retain enough
ephemeral session evidence to answer a dispute during the active dispute window,
but they must not retain user prompts or outputs beyond the privacy rules below.

## 4. Users And Sessions

Users must select from canonical contract rooms and must verify that every
session references an active room, active provider serve record, active enclave,
current rules version, and active price version. A user should abort the session
before spend if any referenced contract state is missing, stale, or mismatched.

Session receipts bind the user, provider, enclave, room, price version, rules
version, token counts, prompt hash, output hash, and spend amount. Receipts are
evidence for settlement and disputes; they are not permission for either party
to publish private session content.

## 5. Pricing And Economy

All canonical prices are denominated in `mu_usd`, integer micro-USD. TNK,
Stripe, Coinbase, and future rails are payment or payout rails around this
shared credit unit; they do not create separate provider-defined prices.

Only the admin may set prices, price bounds, rate windows, fees, holdbacks,
payout minimums, or other economy-critical params. Price changes are
forward-facing. A session uses the price version pinned at session start, and
later price changes must not mutate already-pinned session or receipt terms.

Providers must not negotiate, advertise, or enforce off-contract prices as
canonical Mayhem terms. Any off-contract offer is non-canonical and may be
treated as misleading behavior if it conflicts with contract state.

## 6. Privacy And Data Handling

Prompts, outputs, tool inputs, and session payloads must travel over direct
encrypted data-plane channels. The contract and settlement evidence should store
hashes, counters, roots, and minimal metadata, not raw prompts or outputs.

Providers must not log, sell, reuse, train on, or disclose user prompts or model
outputs except where the user explicitly requests storage as part of the
session. Providers may keep hashes, counters, signatures, and encrypted
short-lived diagnostic material needed for settlement, fraud proofs, uptime
checks, or disputes. Raw diagnostic material must be deleted when the dispute
window ends unless a dispute requires longer retention.

Users must not upload data they do not have the right to process. Auditors and
admins must handle any dispute evidence under the same confidentiality standard.

## 7. Evidence, Settlement, And Receipts

The hot path must avoid per-request ledger writes. Sessions exchange signed
receipts off-chain, and epoch rollers or approved evidence features aggregate
receipts into roots and balance deltas. Participants must not forge receipts,
double-settle usage, omit required evidence, tamper with epoch roots, or submit
evidence they know to be false.

If a receipt, epoch root, payout confirmation, deposit event, or dispute record
is inconsistent, the conservative rule applies: balances must not go negative,
provider earnings must not exceed verified user spend, and settlement must fail
closed until evidence is corrected or a dispute resolves it.

## 8. Slashing, Bans, And Reputation

At launch, the admin may ban a provider from future serving mutations when
necessary to protect users, evidence integrity, network availability, or the
canonical catalog. Later phases may add automated holdbacks, slashing, and
reputation deductions. Slashable or ban-worthy conduct includes:

- serving a non-canonical model, enclave, binary, or room as canonical;
- falsifying attestation, capacity, saturation, uptime, location, or price data;
- refusing to provide required dispute evidence;
- forging, replaying, withholding, or tampering with receipts or epoch evidence;
- retaining or disclosing prompts or outputs in violation of these rules;
- attempting to bypass admin-set prices, params, rules, bans, or room authority;
- attacking the network, peers, users, auditors, payment rails, or settlement
  process.

Bans and slashing should be recorded with a reason hash where practical. A ban
blocks future provider serving actions but must not block withdrawal of funds
that are not frozen by a dispute, fraud proof, holdback, or lawful requirement.

## 9. Disputes

A user, provider, auditor, paygate, or admin may open a dispute when evidence,
service delivery, settlement, payout, or rule compliance is contested. The party
opening the dispute should provide hashes, receipts, session identifiers, room
and enclave references, timestamps, and any encrypted evidence bundle required
by the current dispute process.

Dispute resolution may void bad receipts, correct balances, freeze or release
funds, reduce reputation, ban a provider, or mark an evidence root invalid.
Dispute reviewers must minimize disclosure of raw prompts or outputs and should
prefer hashes, signatures, and reproducible checks whenever possible.

## 10. Auditor Conduct

Auditors must act as neutral testers. They may run canaries, uptime checks,
attestation checks, receipt recomputation, and evidence-root validation. Auditors
must not induce users or providers to violate these rules, leak private session
content, submit fabricated probe results, or use audit access for competitive or
personal advantage. A key registered as a provider must not register as an
auditor or submit auditor probes.

Audit findings must reference canonical contract state and verifiable evidence.
Auditors who submit false or reckless findings may lose auditor eligibility and
may be subject to dispute or reputation penalties.

## 11. Amendments And Re-Consent

The admin may publish a new rules version by setting `rules/<ver>` and
`rules/current` in the contract to the BLAKE3 hash of the new `RULES.md`.
Non-emergency amendments should be announced before activation. When the active
rules version changes, participants must review the new text and sign fresh
consent before performing gated earning, usage, or auditor actions.

Withdrawals and exits should remain available even when a participant has stale
consent, except where funds are frozen by an active dispute, fraud proof,
holdback, or lawful requirement. Consent gates participation and earning; it is
not a trapdoor that prevents exit.

## 12. Launch Trust Assumptions

Mayhem v1 launches with an admin-owned catalog and economy so users can know
which enclaves, models, rooms, and prices are canonical. The roadmap includes
stronger decentralization of governance, admin custody, auditing, and automated
enforcement. Until those later phases are complete, participants consent to the
admin authority described in these rules and recorded in the contract.

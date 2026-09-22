# OpenMayhem Core 0.2.243

Tier 4 provider identity is now independent from execution attestation. Routing can
require verified Tier 4 accountability while session admission continues to verify
the provider's underlying Tier 1, Tier 2, or Tier 3 evidence. Tier 2 and Tier 3
policies remain fail-closed.

Contract version 26 adds an administrator-audited recovery path for provider KYB
revocations that were explicitly recorded as reversible. Recovery requires the
original provider identity and all three bound KYB ban indexes; it does not weaken
provider, device, fingerprint, or committer bans. Re-verification still requires a
valid administrator signature.

The exact version 25 contract implementation is retained for authenticated canonical
history replay. Existing schema-11 receipt evidence from versions 23 through 25
remains recoverable without rewriting signatures or billing.

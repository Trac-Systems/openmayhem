# OpenMayhem Core 0.2.242

Verified provider identity and execution attestation are now represented
separately throughout gateway routing. A verified identity can advertise Tier 4
accountability while the gateway still verifies the provider's underlying Tier
1, Tier 2, or Tier 3 execution evidence.

This preserves Tier 2 and Tier 3 policy enforcement and prevents valid provider
reports from being rejected because their execution tier does not equal the
identity tier.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged; no recalibration is required.

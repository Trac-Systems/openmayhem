# OpenMayhem 0.2.265

US Stripe Connect onboarding now requests both `card_payments` and `transfers`, as required for US full-service accounts. Mayhem continues using connected accounts for provider payouts. Existing accounts are reused, other countries retain their existing capability requests, and payout readiness still depends on submitted details, enabled payouts and active transfers.

`mayhem provider stripe adopt --country US` now bounds its signed consent to the service's existing ten-minute limit, independently of the default fifteen-minute readiness wait. Previously, the default command was rejected as an invalid service request signature before reaching Stripe. Shorter explicitly selected timeouts remain supported.

This is a source release for testing. The adoption correction is in the CLI; the onboarding correction is in paygate and takes effect only when the paygate operator updates that service. Publishing this release does not update running services. There are no contract, catalog, inference or settlement changes.

Validation covers US account creation and reuse, unchanged non-US onboarding, payout readiness without card charging, and accepted/rejected signed consent lifetimes. Stripe account eligibility and live onboarding still require confirmation by the affected provider.

The capability choice preserves Stripe’s supported [Connect cross-border payout flow](https://docs.stripe.com/connect/cross-border-payouts); it does not switch accounts to the separate recipient service agreement.

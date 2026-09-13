# Market and Pricing

How OpenMayhem sets prices: one automated, activity-momentum clearing price per market per epoch,
seeded once by the admin and then floated by the contract on verified settled work. Nobody types
the running price.

* [Pricing controller](/market/pricing-controller.md) - the activity-momentum controller: calibrated work, the previous-epoch baseline, the per-epoch step, and hard reference bounds.
* [Reservation bands: min-ask and max-bid](/market/bands-min-ask-max-bid.md) - how providers and users gate participation without naming a price.
* [Epochs, settlement, and price provenance](/market/epochs-and-settlement.md) - the hourly epoch, the price lock, and how every published price carries a recomputable derivation.

Denomination and units: prices are in `au_usd` (atto-USD, 1e18 = $1), rail-agnostic, expressed as a
per-model `rate_map` of `{unit, per_unit_au, granularity}`. See [Glossary](/glossary.md) for terms and
[The Utilization-Indexed Pricing Controller](/market/pricing-controller.md) §units for the full unit table.

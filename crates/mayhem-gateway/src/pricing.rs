use mayhem_proto::{
    MoneyAu, ReceiptUsage, USAGE_CACHED_INPUT_TOKEN, USAGE_INPUT_TOKEN, USAGE_OUTPUT_TOKEN,
};

pub use mayhem_proto::RateMapEntry;

pub const INPUT_TOKEN_UNIT: &str = USAGE_INPUT_TOKEN;
pub const CACHED_INPUT_TOKEN_UNIT: &str = USAGE_CACHED_INPUT_TOKEN;
pub const OUTPUT_TOKEN_UNIT: &str = USAGE_OUTPUT_TOKEN;
pub const CACHED_INPUT_TOKEN_RATE_BPS: u64 = 2_500;

pub fn text_generation_rate_map(
    in_per_1k_au: MoneyAu,
    out_per_1k_au: MoneyAu,
) -> Vec<RateMapEntry> {
    vec![
        RateMapEntry {
            unit: INPUT_TOKEN_UNIT.to_owned(),
            per_unit_au: in_per_1k_au,
            granularity: 1_000,
        },
        RateMapEntry {
            unit: CACHED_INPUT_TOKEN_UNIT.to_owned(),
            per_unit_au: discounted_cached_input_rate_au(in_per_1k_au),
            granularity: 1_000,
        },
        RateMapEntry {
            unit: OUTPUT_TOKEN_UNIT.to_owned(),
            per_unit_au: out_per_1k_au,
            granularity: 1_000,
        },
    ]
}

pub fn discounted_cached_input_rate_au(in_per_1k_au: MoneyAu) -> MoneyAu {
    if in_per_1k_au == 0 {
        return 0;
    }
    ceil_div_u128(
        in_per_1k_au.saturating_mul(u128::from(CACHED_INPUT_TOKEN_RATE_BPS)),
        10_000,
    )
    .max(1)
}

pub fn normalize_rate_map(mut rate_map: Vec<RateMapEntry>) -> Vec<RateMapEntry> {
    rate_map.sort_by(|left, right| left.unit.cmp(&right.unit));
    rate_map
}

pub fn rate_for_unit<'a>(rate_map: &'a [RateMapEntry], unit: &str) -> Option<&'a RateMapEntry> {
    rate_map.iter().find(|entry| entry.unit == unit)
}

pub fn text_rate_per_1k_au(rate_map: &[RateMapEntry], unit: &str) -> MoneyAu {
    rate_for_unit(rate_map, unit)
        .map(|entry| {
            if entry.granularity == 0 {
                0
            } else {
                ceil_div_u128(
                    entry.per_unit_au.saturating_mul(1_000),
                    u128::from(entry.granularity),
                )
            }
        })
        .unwrap_or(0)
}

pub fn text_usage_au(rate_map: &[RateMapEntry], usage: &ReceiptUsage) -> MoneyAu {
    usage_map_au(rate_map, usage)
}

pub fn priced_usage_au(
    rate_map: &[RateMapEntry],
    per_req_au: MoneyAu,
    min_session_au: MoneyAu,
    usage: &ReceiptUsage,
) -> MoneyAu {
    usage_map_au(rate_map, usage)
        .saturating_add(per_req_au)
        .max(min_session_au)
}

pub fn usage_map_au(rate_map: &[RateMapEntry], usage: &ReceiptUsage) -> MoneyAu {
    let counts = usage
        .units()
        .iter()
        .map(|(unit, count)| (unit.as_str(), *count))
        .collect::<Vec<_>>();
    usage_units_au(rate_map, &counts)
}

pub fn text_units_au(rate_map: &[RateMapEntry], in_tokens: u64, out_tokens: u64) -> MoneyAu {
    usage_units_au(
        rate_map,
        &[
            (INPUT_TOKEN_UNIT, in_tokens),
            (OUTPUT_TOKEN_UNIT, out_tokens),
        ],
    )
}

pub fn usage_units_au(rate_map: &[RateMapEntry], counts: &[(&str, u64)]) -> MoneyAu {
    let mut priced = counts
        .iter()
        .filter_map(|(unit, count)| {
            let rate = rate_for_unit(rate_map, unit)?;
            if *count == 0 || rate.per_unit_au == 0 || rate.granularity == 0 {
                return None;
            }
            Some((u128::from(*count), rate.per_unit_au, rate.granularity))
        })
        .collect::<Vec<_>>();

    if priced.is_empty() {
        return 0;
    }

    priced.sort_by_key(|(_, _, granularity)| *granularity);
    if priced
        .first()
        .is_some_and(|(_, _, granularity)| priced.iter().all(|entry| entry.2 == *granularity))
    {
        let granularity = u128::from(priced[0].2);
        let raw = priced.iter().fold(0u128, |acc, (count, per_unit, _)| {
            acc.saturating_add(count.saturating_mul(*per_unit))
        });
        return ceil_div_u128(raw, granularity);
    }

    priced
        .iter()
        .fold(0u128, |acc, (count, per_unit, granularity)| {
            acc.saturating_add(ceil_div_u128(
                count.saturating_mul(*per_unit),
                u128::from(*granularity),
            ))
        })
}

pub fn rate_map_cost_basis_per_1k(rate_map: &[RateMapEntry]) -> MoneyAu {
    rate_map.iter().fold(0u128, |acc, entry| {
        if entry.granularity == 0 {
            acc
        } else {
            acc.saturating_add(ceil_div_u128(
                entry.per_unit_au.saturating_mul(1_000),
                u128::from(entry.granularity),
            ))
        }
    })
}

fn ceil_div_u128(value: u128, divisor: u128) -> u128 {
    if value == 0 || divisor == 0 {
        0
    } else {
        value.div_ceil(divisor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mayhem_proto::{
        ReceiptUsage, USAGE_AUDIO_SECOND, USAGE_CACHED_INPUT_TOKEN, USAGE_IMAGE,
        USAGE_INPUT_CHARACTER, USAGE_STEP,
    };

    #[test]
    fn text_rate_map_matches_legacy_two_token_charge() {
        let rate_map = text_generation_rate_map(20, 60);
        let usage = ReceiptUsage::text(100, 250);

        assert_eq!(usage_map_au(&rate_map, &usage), 17);
    }

    #[test]
    fn text_rate_map_discounts_cached_input_tokens() {
        let rate_map = text_generation_rate_map(20, 60);
        let usage = ReceiptUsage::text_with_cached(100, 400, 250);

        assert_eq!(
            rate_for_unit(&rate_map, USAGE_CACHED_INPUT_TOKEN)
                .unwrap()
                .per_unit_au,
            5
        );
        assert_eq!(usage_map_au(&rate_map, &usage), 19);
    }

    #[test]
    fn image_usage_aliases_settle_against_canonical_units() {
        let usage: ReceiptUsage = serde_json::from_value(serde_json::json!({
            "images": 2,
            "steps": 60
        }))
        .unwrap();
        let rate_map = vec![
            RateMapEntry {
                unit: USAGE_IMAGE.to_owned(),
                per_unit_au: 500,
                granularity: 1,
            },
            RateMapEntry {
                unit: USAGE_STEP.to_owned(),
                per_unit_au: 2,
                granularity: 1,
            },
        ];

        assert_eq!(
            serde_json::to_value(&usage).unwrap(),
            serde_json::json!({ "image": 2, "step": 60 })
        );
        assert_eq!(usage_map_au(&rate_map, &usage), 1_120);
    }

    #[test]
    fn audio_usage_aliases_settle_against_canonical_units() {
        let usage: ReceiptUsage = serde_json::from_value(serde_json::json!({
            "input_chars": 12,
            "audio_seconds": 3
        }))
        .unwrap();
        let rate_map = vec![
            RateMapEntry {
                unit: USAGE_INPUT_CHARACTER.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: 100,
                granularity: 1,
            },
        ];

        assert_eq!(
            serde_json::to_value(&usage).unwrap(),
            serde_json::json!({ "audio_second": 3, "input_character": 12 })
        );
        assert_eq!(usage_map_au(&rate_map, &usage), 312);
    }

    #[test]
    fn embedding_per_token_atto_price_stays_nonzero() {
        let rate_map = vec![RateMapEntry {
            unit: USAGE_INPUT_TOKEN.to_owned(),
            per_unit_au: 10_000_000,
            granularity: 1,
        }];

        assert_eq!(
            usage_units_au(&rate_map, &[(USAGE_INPUT_TOKEN, 1)]),
            10_000_000
        );
        assert_eq!(
            usage_map_au(&rate_map, &ReceiptUsage::text(3, 0)),
            30_000_000
        );
    }
}

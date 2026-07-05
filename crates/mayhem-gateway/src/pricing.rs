use mayhem_proto::ReceiptUsage;
use serde::{Deserialize, Serialize};

pub const INPUT_TOKEN_UNIT: &str = "input_token";
pub const OUTPUT_TOKEN_UNIT: &str = "output_token";

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RateMapEntry {
    pub unit: String,
    pub per_unit_mu: u64,
    pub granularity: u64,
}

pub fn text_generation_rate_map(in_per_1k_mu: u64, out_per_1k_mu: u64) -> Vec<RateMapEntry> {
    vec![
        RateMapEntry {
            unit: INPUT_TOKEN_UNIT.to_owned(),
            per_unit_mu: in_per_1k_mu,
            granularity: 1_000,
        },
        RateMapEntry {
            unit: OUTPUT_TOKEN_UNIT.to_owned(),
            per_unit_mu: out_per_1k_mu,
            granularity: 1_000,
        },
    ]
}

pub fn normalize_rate_map(mut rate_map: Vec<RateMapEntry>) -> Vec<RateMapEntry> {
    rate_map.sort_by(|left, right| left.unit.cmp(&right.unit));
    rate_map
}

pub fn rate_for_unit<'a>(rate_map: &'a [RateMapEntry], unit: &str) -> Option<&'a RateMapEntry> {
    rate_map.iter().find(|entry| entry.unit == unit)
}

pub fn text_rate_per_1k_mu(rate_map: &[RateMapEntry], unit: &str) -> u64 {
    rate_for_unit(rate_map, unit)
        .map(|entry| {
            if entry.granularity == 0 {
                0
            } else {
                ceil_div_u128(
                    u128::from(entry.per_unit_mu).saturating_mul(1_000),
                    u128::from(entry.granularity),
                )
                .min(u128::from(u64::MAX)) as u64
            }
        })
        .unwrap_or(0)
}

pub fn text_usage_mu(rate_map: &[RateMapEntry], usage: &ReceiptUsage) -> u64 {
    usage_units_mu(
        rate_map,
        &[
            (INPUT_TOKEN_UNIT, usage.in_tokens),
            (OUTPUT_TOKEN_UNIT, usage.out_tokens),
        ],
    )
}

pub fn usage_units_mu(rate_map: &[RateMapEntry], counts: &[(&str, u64)]) -> u64 {
    let mut priced = counts
        .iter()
        .filter_map(|(unit, count)| {
            let rate = rate_for_unit(rate_map, unit)?;
            if *count == 0 || rate.per_unit_mu == 0 || rate.granularity == 0 {
                return None;
            }
            Some((
                u128::from(*count),
                u128::from(rate.per_unit_mu),
                rate.granularity,
            ))
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
        return ceil_div_u128(raw, granularity).min(u128::from(u64::MAX)) as u64;
    }

    priced
        .iter()
        .fold(0u128, |acc, (count, per_unit, granularity)| {
            acc.saturating_add(ceil_div_u128(
                count.saturating_mul(*per_unit),
                u128::from(*granularity),
            ))
        })
        .min(u128::from(u64::MAX)) as u64
}

pub fn rate_map_cost_basis_per_1k(rate_map: &[RateMapEntry]) -> u64 {
    rate_map
        .iter()
        .fold(0u128, |acc, entry| {
            if entry.granularity == 0 {
                acc
            } else {
                acc.saturating_add(ceil_div_u128(
                    u128::from(entry.per_unit_mu).saturating_mul(1_000),
                    u128::from(entry.granularity),
                ))
            }
        })
        .min(u128::from(u64::MAX)) as u64
}

fn ceil_div_u128(value: u128, divisor: u128) -> u128 {
    if value == 0 || divisor == 0 {
        0
    } else {
        value.div_ceil(divisor)
    }
}

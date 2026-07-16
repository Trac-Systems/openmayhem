use mayhem_proto::{
    MoneyAu, ReceiptUsage, USAGE_AUDIO_SECOND, USAGE_CACHED_INPUT_TOKEN, USAGE_FRAME, USAGE_IMAGE,
    USAGE_INPUT_CHARACTER, USAGE_INPUT_TOKEN, USAGE_OUTPUT_TOKEN, USAGE_STEP, USAGE_VIDEO_SECOND,
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

/// Returns the cumulative usage a cancelled, already-dispatched request must settle.
///
/// Fixed floors and previously metered work remain authoritative. For a variable-only
/// schedule with no metered progress yet, both peers derive one deterministic work quantum
/// from the signed rate map. This bounds a disconnect charge while preventing free repeated
/// dispatch-and-cancel work.
pub fn cancellation_settlement_usage(
    rate_map: &[RateMapEntry],
    per_req_au: MoneyAu,
    min_session_au: MoneyAu,
    metered_usage: &ReceiptUsage,
) -> Option<ReceiptUsage> {
    if priced_usage_au(rate_map, per_req_au, min_session_au, metered_usage) > 0 {
        return Some(metered_usage.clone());
    }

    let rate = rate_map
        .iter()
        .filter(|rate| rate.per_unit_au > 0 && rate.granularity > 0)
        .min_by(|left, right| {
            cancellation_unit_rank(&left.unit)
                .cmp(&cancellation_unit_rank(&right.unit))
                .then_with(|| {
                    ceil_div_u128(left.per_unit_au, u128::from(left.granularity)).cmp(
                        &ceil_div_u128(right.per_unit_au, u128::from(right.granularity)),
                    )
                })
                .then_with(|| left.unit.cmp(&right.unit))
        })?;
    let usage = ReceiptUsage::from_units([(rate.unit.clone(), 1)]);
    (priced_usage_au(rate_map, per_req_au, min_session_au, &usage) > 0).then_some(usage)
}

fn cancellation_unit_rank(unit: &str) -> u8 {
    match unit {
        // These are the smallest bounded compute units for their model classes.
        USAGE_STEP | USAGE_AUDIO_SECOND | USAGE_FRAME => 0,
        USAGE_INPUT_TOKEN | USAGE_VIDEO_SECOND => 1,
        USAGE_INPUT_CHARACTER => 2,
        USAGE_OUTPUT_TOKEN | USAGE_IMAGE => 3,
        USAGE_CACHED_INPUT_TOKEN => 4,
        _ => 5,
    }
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

/// Scalar rate-band basis: one standardized 1,000-unit basket of every priced unit.
/// Request volume never enters this value. Fixed-only schedules retain a deterministic fallback.
pub fn rate_gate_basis_au(
    rate_map: &[RateMapEntry],
    per_req_au: MoneyAu,
    min_session_au: MoneyAu,
) -> MoneyAu {
    let rate_basis = rate_map_cost_basis_per_1k(rate_map);
    if rate_basis == 0 {
        per_req_au.max(min_session_au)
    } else {
        rate_basis
    }
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
    fn rate_gate_basis_is_independent_of_request_volume() {
        let rate_map = text_generation_rate_map(20, 60);

        assert_eq!(rate_gate_basis_au(&rate_map, 7, 11), 85);
        assert_eq!(rate_gate_basis_au(&[], 7, 11), 11);
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
    fn variable_only_cancellation_uses_one_deterministic_work_quantum() {
        let image_rates = vec![
            RateMapEntry {
                unit: USAGE_IMAGE.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            RateMapEntry {
                unit: USAGE_STEP.to_owned(),
                per_unit_au: 2_499_999_999_999_999,
                granularity: 36,
            },
        ];
        let usage = cancellation_settlement_usage(&image_rates, 0, 0, &ReceiptUsage::default())
            .expect("signed image rate map has a cancellation quantum");

        assert_eq!(usage, ReceiptUsage::from_units([(USAGE_STEP, 1)]));
        assert_eq!(
            priced_usage_au(&image_rates, 0, 0, &usage),
            69_444_444_444_445
        );
    }

    #[test]
    fn cancellation_quantum_covers_every_canonical_modality_rate_family() {
        let rate = |unit: &str, per_unit_au: MoneyAu, granularity: u64| RateMapEntry {
            unit: unit.to_owned(),
            per_unit_au,
            granularity,
        };
        let cases = [
            (
                vec![
                    rate(USAGE_OUTPUT_TOKEN, 60, 1_000),
                    rate(USAGE_CACHED_INPUT_TOKEN, 5, 1_000),
                    rate(USAGE_INPUT_TOKEN, 20, 1_000),
                ],
                USAGE_INPUT_TOKEN,
            ),
            (
                vec![
                    rate(USAGE_AUDIO_SECOND, 18_000_000_000_000, 1),
                    rate(USAGE_INPUT_CHARACTER, 1, 1),
                ],
                USAGE_AUDIO_SECOND,
            ),
            (
                vec![rate(USAGE_AUDIO_SECOND, 1_000_000_000_000_000, 60)],
                USAGE_AUDIO_SECOND,
            ),
            (
                vec![
                    rate(USAGE_VIDEO_SECOND, 5_000_000_000_000_000, 1),
                    rate(USAGE_FRAME, 5_000_000_000_000_000, 24),
                ],
                USAGE_FRAME,
            ),
        ];

        for (rate_map, expected_unit) in cases {
            let usage = cancellation_settlement_usage(&rate_map, 0, 0, &ReceiptUsage::default())
                .expect("canonical rate map has a cancellation quantum");
            assert_eq!(usage, ReceiptUsage::from_units([(expected_unit, 1)]));
            assert!(priced_usage_au(&rate_map, 0, 0, &usage) > 0);
        }
    }

    #[test]
    fn cancellation_preserves_fixed_floors_and_metered_progress() {
        let text_rates = text_generation_rate_map(20, 60);
        let empty = ReceiptUsage::default();
        assert_eq!(
            cancellation_settlement_usage(&text_rates, 11, 37, &empty),
            Some(empty)
        );

        let metered = ReceiptUsage::text(100, 4);
        assert_eq!(
            cancellation_settlement_usage(&text_rates, 0, 0, &metered),
            Some(metered)
        );
        assert!(cancellation_settlement_usage(&[], 0, 0, &ReceiptUsage::default()).is_none());
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

    #[test]
    fn non_llm_roster_rate_maps_preserve_canonical_start_prices() {
        let image_rate_map = |step_rate_au, step_granularity| {
            vec![
                RateMapEntry {
                    unit: USAGE_IMAGE.to_owned(),
                    per_unit_au: 1,
                    granularity: 1,
                },
                RateMapEntry {
                    unit: USAGE_STEP.to_owned(),
                    per_unit_au: step_rate_au,
                    granularity: step_granularity,
                },
            ]
        };
        for (rate_map, metered_steps, expected_au) in [
            (
                image_rate_map(2_499_999_999_999_999, 36),
                36,
                2_500_000_000_000_000,
            ),
            (
                image_rate_map(2_499_999_999_999_999, 16),
                16,
                2_500_000_000_000_000,
            ),
            (
                image_rate_map(4_999_999_999_999_999, 160),
                160,
                5_000_000_000_000_000,
            ),
            (
                image_rate_map(7_499_999_999_999_999, 200),
                200,
                7_500_000_000_000_000,
            ),
        ] {
            assert_eq!(
                usage_units_au(&rate_map, &[(USAGE_IMAGE, 1), (USAGE_STEP, metered_steps)]),
                expected_au
            );
        }

        let stt_rate_map = |per_minute_au| {
            vec![RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: per_minute_au,
                granularity: 60,
            }]
        };
        assert_eq!(
            usage_units_au(
                &stt_rate_map(1_500_000_000_000_000),
                &[(USAGE_AUDIO_SECOND, 60)]
            ),
            1_500_000_000_000_000
        );
        assert_eq!(
            usage_units_au(
                &stt_rate_map(1_000_000_000_000_000),
                &[(USAGE_AUDIO_SECOND, 60)]
            ),
            1_000_000_000_000_000
        );

        let kokoro_rate_map = vec![
            RateMapEntry {
                unit: USAGE_INPUT_CHARACTER.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: 18_000_000_000_000,
                granularity: 1,
            },
        ];
        assert_eq!(
            usage_units_au(
                &kokoro_rate_map,
                &[(USAGE_INPUT_CHARACTER, 12), (USAGE_AUDIO_SECOND, 1)]
            ),
            18_000_000_000_012
        );
        let chatterbox_rate_map = vec![
            RateMapEntry {
                unit: USAGE_INPUT_CHARACTER.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: 60_000_000_000_000,
                granularity: 1,
            },
        ];
        assert_eq!(
            usage_units_au(
                &chatterbox_rate_map,
                &[(USAGE_INPUT_CHARACTER, 12), (USAGE_AUDIO_SECOND, 1)]
            ),
            60_000_000_000_012
        );

        let ace_step_rate_map = vec![
            RateMapEntry {
                unit: USAGE_INPUT_CHARACTER.to_owned(),
                per_unit_au: 1,
                granularity: 1,
            },
            RateMapEntry {
                unit: USAGE_AUDIO_SECOND.to_owned(),
                per_unit_au: 100_000_000_000_000,
                granularity: 1,
            },
        ];
        assert_eq!(
            usage_units_au(
                &ace_step_rate_map,
                &[(USAGE_INPUT_CHARACTER, 0), (USAGE_AUDIO_SECOND, 100)]
            ),
            10_000_000_000_000_000
        );

        let embedding_rate_map = vec![RateMapEntry {
            unit: USAGE_INPUT_TOKEN.to_owned(),
            per_unit_au: 4_000_000_000_000_000,
            granularity: 1_000_000,
        }];
        assert_eq!(
            usage_units_au(&embedding_rate_map, &[(USAGE_INPUT_TOKEN, 1_000_000)]),
            4_000_000_000_000_000
        );
        let reranker_rate_map = vec![RateMapEntry {
            unit: USAGE_INPUT_TOKEN.to_owned(),
            per_unit_au: 10_000_000_000_000_000,
            granularity: 1_000_000,
        }];
        assert_eq!(
            usage_units_au(&reranker_rate_map, &[(USAGE_INPUT_TOKEN, 1_000_000)]),
            10_000_000_000_000_000
        );
    }
}

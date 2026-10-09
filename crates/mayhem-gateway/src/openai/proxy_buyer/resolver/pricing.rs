//! Exact SITE retail projection. This does not alter wholesale accepted terms.
use mayhem_proto::{
    proxy::{ProxyOffer, PROXY_MAX_SAFE_INTEGER},
    MoneyAu,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetailRanking {
    pub margin_bps: u64,
    #[serde(with = "mayhem_proto::decimal_u128")]
    pub max_cost_micro: MoneyAu,
}
impl RetailRanking {
    pub fn valid(&self, profile_cap: MoneyAu) -> bool {
        self.margin_bps <= PROXY_MAX_SAFE_INTEGER
            && self.max_cost_micro > 0
            && self.max_cost_micro <= profile_cap
    }
    pub fn maximum(&self, offer: &ProxyOffer, usage: &BTreeMap<String, u64>) -> Option<MoneyAu> {
        let au = total(offer, usage, self.margin_bps)?;
        let micro = au.div_ceil(1_000_000_000_000);
        (micro <= self.max_cost_micro).then_some(micro)
    }
}
fn total(offer: &ProxyOffer, usage: &BTreeMap<String, u64>, margin: u64) -> Option<MoneyAu> {
    if margin > PROXY_MAX_SAFE_INTEGER || usage.len() != offer.rates.len() {
        return None;
    }
    let marked = |amount| Wide::from(amount).mul(10_000 + margin)?.ceil_div(10_000);
    let mut total = marked(offer.per_request_au)?.amount()?;
    for rate in &offer.rates {
        let units = *usage.get(&rate.unit)?;
        if units > PROXY_MAX_SAFE_INTEGER
            || rate.granularity == 0
            || rate.granularity > PROXY_MAX_SAFE_INTEGER
        {
            return None;
        }
        let cost = marked(rate.per_unit_au)?
            .mul(units)?
            .ceil_div(rate.granularity)?
            .amount()?;
        total = total.checked_add(cost)?;
    }
    Some(total.max(marked(offer.min_session_au)?.amount()?))
}

// Four limbs are sufficient: a u128 amount times two safe JSON integers is
// below 2^235. Long division keeps each intermediate within u128. This avoids
// overflow from a multiply-before-divide while matching JavaScript BigInt.
#[derive(Clone, Copy)]
struct Wide([u64; 4]);
impl From<u128> for Wide {
    fn from(v: u128) -> Self {
        Self([v as u64, (v >> 64) as u64, 0, 0])
    }
}
impl Wide {
    fn mul(self, value: u64) -> Option<Self> {
        let mut out = [0; 4];
        let mut carry = 0u128;
        for (i, limb) in self.0.iter().enumerate() {
            let next = u128::from(*limb) * u128::from(value) + carry;
            out[i] = next as u64;
            carry = next >> 64;
        }
        (carry == 0).then_some(Self(out))
    }
    fn ceil_div(self, value: u64) -> Option<Self> {
        if value == 0 {
            return None;
        }
        let mut out = [0; 4];
        let mut remainder = 0u128;
        for i in (0..4).rev() {
            let next = (remainder << 64) | u128::from(self.0[i]);
            out[i] = (next / u128::from(value)) as u64;
            remainder = next % u128::from(value);
        }
        if remainder != 0 {
            let mut carry = true;
            for limb in &mut out {
                if carry {
                    let (next, overflow) = limb.overflowing_add(1);
                    *limb = next;
                    carry = overflow;
                }
            }
            if carry {
                return None;
            }
        }
        Some(Self(out))
    }
    fn amount(self) -> Option<u128> {
        (self.0[2] == 0 && self.0[3] == 0)
            .then_some(u128::from(self.0[0]) | (u128::from(self.0[1]) << 64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_wide_intermediates_zero_usage_and_overflow_match_retail_limits() {
        assert_eq!(
            Wide::from(u128::MAX)
                .mul(10_000)
                .unwrap()
                .ceil_div(10_000)
                .unwrap()
                .amount(),
            Some(u128::MAX)
        );
        assert_eq!(
            Wide::from(u128::MAX)
                .mul(PROXY_MAX_SAFE_INTEGER)
                .unwrap()
                .ceil_div(PROXY_MAX_SAFE_INTEGER)
                .unwrap()
                .amount(),
            Some(u128::MAX)
        );
        assert_eq!(
            Wide::from(u128::MAX)
                .mul(10_001)
                .unwrap()
                .ceil_div(10_000)
                .unwrap()
                .amount(),
            None
        );
        assert_eq!(
            Wide::from(u128::MAX)
                .mul(PROXY_MAX_SAFE_INTEGER)
                .unwrap()
                .mul(0)
                .unwrap()
                .amount(),
            Some(0)
        );
        assert_eq!(
            Wide::from(1)
                .mul(10_001)
                .unwrap()
                .ceil_div(10_000)
                .unwrap()
                .amount(),
            Some(2)
        );
        for numerator in [
            0,
            1,
            9999,
            10000,
            10001,
            u64::MAX as u128,
            (u64::MAX as u128) + 1,
        ] {
            for multiplier in [0, 1, 10_001, PROXY_MAX_SAFE_INTEGER] {
                for denominator in [1, 10_000, PROXY_MAX_SAFE_INTEGER] {
                    assert_eq!(
                        Wide::from(numerator)
                            .mul(multiplier)
                            .unwrap()
                            .ceil_div(denominator)
                            .unwrap()
                            .amount(),
                        Some(
                            (numerator * u128::from(multiplier)).div_ceil(u128::from(denominator))
                        )
                    );
                }
            }
        }
    }
    #[test]
    fn per_rate_rounding_can_reverse_wholesale_order_at_retail_micro_precision() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mayhem-proto/tests/fixtures/proxy-wire-v1.json"
        )))
        .unwrap();
        let mut a: ProxyOffer =
            serde_json::from_value(fixture["cases"][0]["offer"].clone()).unwrap();
        a.per_request_au = 0;
        a.min_session_au = 0;
        a.rates = vec![mayhem_proto::proxy::ProxyRate {
            unit: "input_tokens".into(),
            per_unit_au: 1,
            granularity: 1,
        }];
        let mut b = a.clone();
        b.rates[0].per_unit_au = 15;
        b.rates[0].granularity = 10;
        let usage = BTreeMap::from([("input_tokens".into(), 1_000_000_000_000)]);
        assert!(a.cost(&usage).unwrap() < b.cost(&usage).unwrap());
        let p = RetailRanking {
            margin_bps: 1,
            max_cost_micro: 100,
        };
        assert_eq!(p.maximum(&a, &usage), Some(2));
        // 1 AU marks up to 2; 15 AU marks up to 16 then divides by 10.
        // Use three trillion units so the rounded micro totals separate.
        let usage = BTreeMap::from([("input_tokens".into(), 3_000_000_000_000)]);
        assert_eq!(p.maximum(&a, &usage), Some(6));
        assert_eq!(p.maximum(&b, &usage), Some(5));
        assert!(p.maximum(&a, &usage) > p.maximum(&b, &usage));
        let capped = RetailRanking {
            max_cost_micro: 5,
            ..p
        };
        assert_eq!(capped.maximum(&a, &usage), None);
        assert_eq!(capped.maximum(&b, &usage), Some(5));
    }
}

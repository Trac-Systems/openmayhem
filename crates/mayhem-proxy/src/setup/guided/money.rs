//! Exact user-facing USD denomination. No float, rounding, FX, or fee defaults.
use super::*;
fn decimal(value: &str, digits: u32) -> Result<u128> {
    let mut parts = value.split('.');
    let whole = parts.next().ok_or(Error::Invalid)?;
    let fraction = parts.next().unwrap_or("");
    require(
        !whole.is_empty()
            && whole.len() <= 39
            && whole.bytes().all(|b| b.is_ascii_digit())
            && (whole.len() == 1 || !whole.starts_with('0'))
            && fraction.len() <= digits as usize
            && fraction.bytes().all(|b| b.is_ascii_digit())
            && parts.next().is_none()
            && (!value.contains('.') || !fraction.is_empty()),
    )?;
    whole
        .parse::<u128>()
        .ok()
        .and_then(|n| n.checked_mul(10u128.pow(digits)))
        .and_then(|n| {
            if fraction.is_empty() {
                Some(0)
            } else {
                fraction.parse::<u128>().ok()
            }
            .and_then(|f| f.checked_mul(10u128.pow(digits - fraction.len() as u32)))
            .and_then(|f| n.checked_add(f))
        })
        .ok_or(Error::Invalid)
}
pub fn usd_to_au(value: &str) -> Result<u128> {
    decimal(value, 18)
}
pub fn usd_to_microusd(value: &str) -> Result<u64> {
    let value = u64::try_from(decimal(value, 6)?).map_err(|_| Error::Invalid)?;
    require(value <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER)?;
    Ok(value)
}
pub fn au_to_usd(value: u128) -> String {
    let whole = value / 1_000_000_000_000_000_000;
    let fraction = value % 1_000_000_000_000_000_000;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!(
            "{whole}.{}",
            format!("{fraction:018}").trim_end_matches('0')
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_human_amounts_never_round_or_infer_policy() {
        for n in [
            0,
            1,
            123,
            999_999_999_999_999_999,
            1_000_000_000_000_000_000,
            u128::MAX,
        ] {
            assert_eq!(usd_to_au(&au_to_usd(n)).unwrap(), n);
        }
        assert_eq!(usd_to_microusd("0.000001").unwrap(), 1);
        assert_eq!(usd_to_microusd("12.345678").unwrap(), 12_345_678);
        for v in [
            "",
            ".1",
            "1.",
            "01",
            "-1",
            "+1",
            "1e6",
            " 1",
            "1.0000000000000000001",
            "340282366920938463463.374607431768211456",
        ] {
            assert!(usd_to_au(v).is_err(), "{v}");
        }
        assert!(usd_to_microusd("0.0000001").is_err());
        assert!(usd_to_microusd("9007199254.740992").is_err());
    }
}

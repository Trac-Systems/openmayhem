use super::*;
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum TypedValue {
    Boolean(bool),
    Enum(String),
    Set(Vec<String>),
    Integer(i64),
    Decimal(String),
    Text(String),
}

const SCALE: i128 = 1_000_000_000_000_000_000;
const MAX_WHOLE: i128 = 999_999_999_999_999_999;
fn decimal(value: &str) -> Result<i128> {
    require(
        !value.is_empty() && value.len() <= 38,
        "invalid registry decimal",
    )?;
    let (negative, absolute) = value
        .strip_prefix('-')
        .map(|v| (true, v))
        .unwrap_or((false, value));
    let mut parts = absolute.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next();
    require(
        parts.next().is_none()
            && !whole.is_empty()
            && whole.len() <= 18
            && whole.bytes().all(|b| b.is_ascii_digit())
            && (whole.len() == 1 || !whole.starts_with('0'))
            && fraction.is_none_or(|f| {
                !f.is_empty()
                    && f.len() <= 18
                    && f.bytes().all(|b| b.is_ascii_digit())
                    && !f.ends_with('0')
            }),
        "registry decimal must be canonical",
    )?;
    let mut number = whole
        .parse::<i128>()
        .map_err(|_| invalid("invalid registry decimal"))?
        * SCALE;
    if let Some(f) = fraction {
        number += f
            .parse::<i128>()
            .map_err(|_| invalid("invalid registry decimal"))?
            * 10i128.pow((18 - f.len()) as u32);
    }
    require(
        !negative || number != 0,
        "negative zero is not a registry decimal",
    )?;
    Ok(if negative { -number } else { number })
}

fn represented_decimal(value: &str) -> Result<i128> {
    let (mantissa, exponent) = if let Some((m, e)) = value.split_once(['e', 'E']) {
        (
            m,
            e.parse::<i32>()
                .map_err(|_| invalid("invalid registry exponent"))?,
        )
    } else {
        (value, 0)
    };
    require(
        (-64..=64).contains(&exponent),
        "decimal cannot be represented by this endpoint",
    )?;
    let (negative, mantissa) = mantissa
        .strip_prefix('-')
        .map(|v| (true, v))
        .unwrap_or((false, mantissa));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    require(
        !digits.is_empty() && digits.len() <= 36 && digits.bytes().all(|b| b.is_ascii_digit()),
        "invalid endpoint decimal",
    )?;
    let mut n = digits
        .parse::<i128>()
        .map_err(|_| invalid("invalid registry decimal"))?;
    let power = 18 + exponent - fraction.len() as i32;
    if power >= 0 {
        n = n
            .checked_mul(
                10i128
                    .checked_pow(power as u32)
                    .ok_or_else(|| invalid("endpoint decimal overflow"))?,
            )
            .ok_or_else(|| invalid("endpoint decimal overflow"))?;
    } else {
        let divisor = 10i128
            .checked_pow((-power) as u32)
            .ok_or_else(|| invalid("endpoint decimal precision"))?;
        require(n % divisor == 0, "endpoint decimal precision")?;
        n /= divisor;
    }
    require(
        n <= MAX_WHOLE * SCALE + SCALE - 1,
        "endpoint decimal outside registry range",
    )?;
    Ok(if negative { -n } else { n })
}

impl TypedValue {
    pub fn validate(&self) -> Result<()> {
        require(
            match self {
                Self::Boolean(_) => true,
                Self::Enum(v) => text(v, 128, 512) && !v.contains("://"),
                Self::Set(v) => {
                    !v.is_empty()
                        && v.len() <= 64
                        && ordered(v)
                        && v.iter().all(|s| text(s, 128, 512) && !s.contains("://"))
                }
                Self::Integer(v) => v.unsigned_abs() <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER,
                Self::Decimal(v) => decimal(v).is_ok(),
                Self::Text(v) => text(v, 512, 512) && !v.contains("://"),
            },
            "invalid registry typed value",
        )
    }
    pub(crate) fn supports(&self, op: Operator) -> bool {
        match op {
            Operator::Eq => true,
            Operator::ContainsAll => matches!(self, Self::Set(_)),
            Operator::Gte | Operator::Lte => matches!(self, Self::Integer(_) | Self::Decimal(_)),
        }
    }
    pub(crate) fn matches(&self, op: Operator, required: &Self) -> Result<bool> {
        self.validate()?;
        required.validate()?;
        require(
            self.supports(op) && required.supports(op),
            "invalid registry comparison",
        )?;
        Ok(match (self, required) {
            (Self::Integer(a), Self::Integer(b)) => match op {
                Operator::Eq => a == b,
                Operator::Gte => a >= b,
                Operator::Lte => a <= b,
                _ => false,
            },
            (Self::Decimal(a), Self::Decimal(b)) => {
                let (a, b) = (decimal(a)?, decimal(b)?);
                match op {
                    Operator::Eq => a == b,
                    Operator::Gte => a >= b,
                    Operator::Lte => a <= b,
                    _ => false,
                }
            }
            (Self::Set(a), Self::Set(b)) if op == Operator::ContainsAll => {
                b.iter().all(|v| a.binary_search(v).is_ok())
            }
            _ if std::mem::discriminant(self) == std::mem::discriminant(required)
                && op == Operator::Eq =>
            {
                self == required
            }
            _ => return Err(invalid("registry value types differ")),
        })
    }
    pub(crate) fn json(&self) -> Result<Value> {
        self.validate()?;
        Ok(match self {
            Self::Boolean(v) => Value::Bool(*v),
            Self::Integer(v) => (*v).into(),
            Self::Enum(v) | Self::Text(v) => v.clone().into(),
            Self::Set(v) => serde_json::to_value(v)?,
            Self::Decimal(v) => {
                let result: Value = serde_json::from_str(v)?;
                require(
                    result.is_number() && represented_decimal(&result.to_string())? == decimal(v)?,
                    "endpoint number would change the selected decimal",
                )?;
                require(
                    result.as_i64().is_none_or(|n| {
                        n.unsigned_abs() <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
                    }) && result
                        .as_u64()
                        .is_none_or(|n| n <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER),
                    "endpoint integer exceeds interoperable JSON precision",
                )?;
                result
            }
        })
    }
}

impl ValueSchema {
    /// Read an explicit public request value through its exact typed definition.
    /// No defaults, string coercion, enum ranking or request mutation.
    pub(super) fn read_json(&self, value: &Value) -> Result<TypedValue> {
        let value = match (self, value) {
            (Self::Boolean, Value::Bool(v)) => TypedValue::Boolean(*v),
            (Self::Enum { .. }, Value::String(v)) => TypedValue::Enum(v.clone()),
            (Self::Text { .. }, Value::String(v)) => TypedValue::Text(v.clone()),
            (Self::Set { .. }, Value::Array(v)) => TypedValue::Set(
                v.iter()
                    .map(|s| {
                        s.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| invalid("invalid request set value"))
                    })
                    .collect::<Result<_>>()?,
            ),
            (Self::Integer { .. }, Value::Number(v)) => TypedValue::Integer(
                v.as_i64()
                    .ok_or_else(|| invalid("request control is not an exact integer"))?,
            ),
            (Self::Decimal { .. }, Value::Number(v)) => {
                let n = represented_decimal(&v.to_string())?;
                let magnitude = n.abs();
                let whole = magnitude / SCALE;
                let fraction = magnitude % SCALE;
                let mut canonical = format!("{}{whole}", if n < 0 { "-" } else { "" });
                if fraction != 0 {
                    canonical.push('.');
                    canonical.push_str(format!("{fraction:018}").trim_end_matches('0'));
                }
                let result = TypedValue::Decimal(canonical);
                result.json()?; // The normal exact/interoperable transport guard.
                result
            }
            _ => return Err(invalid("request control value has the wrong type")),
        };
        self.accepts(&value)?;
        Ok(value)
    }

    pub(super) fn validate(&self) -> Result<()> {
        require(
            match self {
                Self::Boolean => true,
                Self::Enum { values } | Self::Set { values } => {
                    !values.is_empty()
                        && values.len() <= 64
                        && ordered(values)
                        && values
                            .iter()
                            .all(|v| text(v, 128, 512) && !v.contains("://"))
                }
                Self::Integer { minimum, maximum } => {
                    minimum <= maximum
                        && minimum.unsigned_abs() <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
                        && maximum.unsigned_abs() <= mayhem_proto::proxy::PROXY_MAX_SAFE_INTEGER
                }
                Self::Decimal { minimum, maximum } => decimal(minimum)? <= decimal(maximum)?,
                Self::Text {
                    min_length,
                    max_length,
                } => *min_length > 0 && min_length <= max_length && *max_length <= 512,
            },
            "invalid registry value schema",
        )
    }
    pub fn accepts(&self, value: &TypedValue) -> Result<()> {
        value.validate()?;
        require(
            match (self, value) {
                (Self::Boolean, TypedValue::Boolean(_)) => true,
                (Self::Enum { values }, TypedValue::Enum(v)) => values.binary_search(v).is_ok(),
                (Self::Set { values }, TypedValue::Set(v)) => {
                    v.iter().all(|v| values.binary_search(v).is_ok())
                }
                (Self::Integer { minimum, maximum }, TypedValue::Integer(v)) => {
                    v >= minimum && v <= maximum
                }
                (Self::Decimal { minimum, maximum }, TypedValue::Decimal(v)) => {
                    let v = decimal(v)?;
                    v >= decimal(minimum)? && v <= decimal(maximum)?
                }
                (
                    Self::Text {
                        min_length,
                        max_length,
                    },
                    TypedValue::Text(v),
                ) => {
                    let n = v.chars().count();
                    n >= *min_length as usize && n <= *max_length as usize
                }
                _ => false,
            },
            "registry value is outside the exact definition",
        )
    }
    pub(super) fn supports(&self, op: Operator) -> bool {
        match op {
            Operator::Eq => true,
            Operator::ContainsAll => matches!(self, Self::Set { .. }),
            Operator::Gte | Operator::Lte => {
                matches!(self, Self::Integer { .. } | Self::Decimal { .. })
            }
        }
    }
}

use rust_decimal::prelude::FromPrimitive;
use serde_json::{json, Value};

/// 2^53 — the largest integer every JSON consumer (LuaJIT doubles, JS
/// numbers) can hold exactly. Bigger values must travel as strings or
/// they silently lose precision.
pub(super) const MAX_SAFE_INT: i64 = 9_007_199_254_740_992;

pub(super) fn opt_json<T: serde::Serialize>(v: Option<T>) -> Value {
    v.map(|v| json!(v)).unwrap_or(Value::Null)
}

/// Serialize an i64 as a JSON number when it fits exactly in a double
/// (|v| < 2^53), otherwise as a JSON string to preserve every digit.
/// Without this, e.g. bigint 2084515900853196878 decodes as
/// 2084515900853196800 on the Lua/JS side.
pub(super) fn int_json(v: i64) -> Value {
    if v > -MAX_SAFE_INT && v < MAX_SAFE_INT {
        json!(v)
    } else {
        json!(v.to_string())
    }
}

pub(super) fn opt_int_json(v: Option<i64>) -> Value {
    v.map(int_json).unwrap_or(Value::Null)
}

/// Serialize a numeric/decimal value as a JSON number only when the
/// double round-trip is exact; otherwise keep the exact decimal string.
/// Large DECIMAL/NUMERIC values otherwise silently lose precision.
pub(super) fn decimal_json(v: rust_decimal::Decimal) -> Value {
    match v.to_string().parse::<f64>() {
        Ok(n) if rust_decimal::Decimal::from_f64(n) == Some(v) => json!(n),
        _ => json!(v.to_string()),
    }
}

pub(super) fn string_fallback(s: Option<String>, b: Option<Vec<u8>>) -> Value {
    if let Some(s) = s {
        json!(s)
    } else if let Some(b) = b {
        json!(String::from_utf8_lossy(&b).to_string())
    } else {
        Value::Null
    }
}

pub(super) fn date_fallback(
    try_date: Option<sqlx::types::chrono::NaiveDate>,
    s: Option<String>,
    b: Option<Vec<u8>>,
) -> Value {
    if let Some(v) = try_date {
        json!(v.format("%Y-%m-%d").to_string())
    } else {
        string_fallback(s, b)
    }
}

pub(super) fn datetime_fallback(
    v: Option<sqlx::types::chrono::NaiveDateTime>,
    s: Option<String>,
    b: Option<Vec<u8>>,
) -> Value {
    if let Some(v) = v {
        json!(v.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
    } else {
        string_fallback(s, b)
    }
}

pub(super) fn timestamptz_fallback(
    v: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    s: Option<String>,
    b: Option<Vec<u8>>,
) -> Value {
    if let Some(v) = v {
        json!(v
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%dT%H:%M:%S%.3f%:z")
            .to_string())
    } else {
        string_fallback(s, b)
    }
}

pub(super) fn time_fallback(
    v: Option<sqlx::types::chrono::NaiveTime>,
    s: Option<String>,
    b: Option<Vec<u8>>,
) -> Value {
    if let Some(v) = v {
        json!(v.format("%H:%M:%S%.3f").to_string())
    } else {
        string_fallback(s, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn int_json_keeps_safe_ints_as_numbers() {
        assert_eq!(int_json(0), json!(0));
        assert_eq!(int_json(42), json!(42));
        assert_eq!(int_json(-42), json!(-42));
        assert_eq!(int_json(MAX_SAFE_INT - 1), json!(MAX_SAFE_INT - 1));
        assert_eq!(int_json(-(MAX_SAFE_INT - 1)), json!(-(MAX_SAFE_INT - 1)));
    }

    #[test]
    fn int_json_serializes_large_ints_as_strings() {
        assert_eq!(int_json(MAX_SAFE_INT), json!("9007199254740992"));
        assert_eq!(int_json(-MAX_SAFE_INT), json!("-9007199254740992"));
        assert_eq!(int_json(i64::MAX), json!(i64::MAX.to_string()));
        assert_eq!(int_json(i64::MIN), json!(i64::MIN.to_string()));
        assert_eq!(
            int_json(2_084_515_900_853_196_878),
            json!("2084515900853196878")
        );
    }

    #[test]
    fn opt_int_json_maps_none_to_null() {
        assert_eq!(opt_int_json(None), Value::Null);
        assert_eq!(opt_int_json(Some(1)), json!(1));
    }

    #[test]
    fn decimal_json_keeps_exact_round_trip_as_number() {
        let d = rust_decimal::Decimal::from_str("123.45").unwrap();
        assert_eq!(decimal_json(d), json!(123.45));
        let i = rust_decimal::Decimal::from_str("42").unwrap();
        assert_eq!(decimal_json(i), json!(42.0));
    }

    #[test]
    fn decimal_json_preserves_large_precision_as_string() {
        let d = rust_decimal::Decimal::from_str("123456789012345678901.50").unwrap();
        assert_eq!(decimal_json(d), json!("123456789012345678901.50"));
    }
}

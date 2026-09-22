//! Per-dialect value converters: one sqlx cell → one wire JSON value.
//!
//! THE live converter: the `session` path owns this logic and `exec-file`
//! must stay in sync with it (docs/schema.md). Both now share this single
//! copy. Types are sqlx row handles; `col_type` is the driver-reported type
//! name of the column (e.g. "NUMERIC", "TIMESTAMPTZ", "BLOB").

use rust_decimal::prelude::FromPrimitive;
use serde_json::{json, Value};

pub fn sqlite_value_to_json(row: &sqlx::sqlite::SqliteRow, idx: usize, _col_type: &str) -> Value {
    use sqlx::{Row, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        return float_json(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if let Some(parsed) = poste_core::sql_parser::parse_json_cell(&v) {
            return parsed;
        }
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        return json!(v);
    }
    Value::Null
}

pub fn pg_value_to_json(row: &sqlx::postgres::PgRow, idx: usize, col_type: &str) -> Value {
    use sqlx::{Row, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    let upper = col_type.to_uppercase();
    match upper.as_str() {
        "NUMERIC" => {
            if let Ok(Some(v)) = row.try_get::<Option<rust_decimal::Decimal>, _>(idx) {
                return match v.to_string().parse::<f64>() {
                    Ok(n) if rust_decimal::Decimal::from_f64(n) == Some(v) => json!(n),
                    _ => json!(v.to_string()),
                };
            }
            return Value::Null;
        }
        "DATE" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDate>, _>(idx) {
                return json!(v.format("%Y-%m-%d").to_string());
            }
        }
        "TIMESTAMP" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
                return json!(v.format("%Y-%m-%d %H:%M:%S%.3f").to_string());
            }
        }
        "TIMESTAMPTZ" | "TIMESTAMP WITH TIME ZONE" => {
            if let Ok(Some(v)) = row
                .try_get::<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>, _>(idx)
            {
                let local = v.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string());
            }
        }
        "TIME" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveTime>, _>(idx) {
                return json!(v.format("%H:%M:%S%.3f").to_string());
            }
        }
        "UUID" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::uuid::Uuid>, _>(idx) {
                return json!(v.to_string());
            }
        }
        "INET" | "CIDR" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::ipnetwork::IpNetwork>, _>(idx) {
                return json!(v.to_string());
            }
        }
        "JSON" | "JSONB" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::Json<Value>>, _>(idx) {
                return v.0;
            }
            if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
                return serde_json::from_str(&s).unwrap_or(json!(s));
            }
            return Value::Null;
        }
        _ => {}
    }

    if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<i16>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        if upper == "INT8" || upper == "BIGINT" {
            let max_safe: i64 = 9_007_199_254_740_992;
            if v > -max_safe && v < max_safe {
                return json!(v);
            } else {
                return json!(v.to_string());
            }
        }
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        return float_json(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if let Some(parsed) = poste_core::sql_parser::parse_json_cell(&v) {
            return parsed;
        }
        if upper == "TIMESTAMPTZ" || upper == "TIMESTAMP WITH TIME ZONE" {
            if let Ok(dt) = v.parse::<chrono::DateTime<chrono::Utc>>() {
                let local = dt.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
            }
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&v, "%Y-%m-%d %H:%M:%S%.f") {
                let utc = dt.and_utc();
                let local = utc.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
            }
        }
        return json!(v);
    }
    Value::Null
}

/// Serialize a float, keeping the values the engines answer with for the three
/// doubles JSON cannot name. `json!(f64::INFINITY)` is `null` (see
/// `a_json_number_cannot_carry_infinity`), so without this an infinite or
/// NaN `float8` cell arrives as NULL and a commit writes NULL over it. The
/// spellings below are what postgres prints and parses back for those values.
pub fn float_json<T: Into<f64> + Copy>(v: T) -> Value {
    let v: f64 = v.into();
    if v.is_finite() {
        return json!(v);
    }
    if v.is_nan() {
        json!("NaN")
    } else if v > 0.0 {
        json!("Infinity")
    } else {
        json!("-Infinity")
    }
}

/// `float_json` for a nullable cell: NULL stays NULL, and the non-finite
/// doubles become the text above rather than NULL by accident.
pub fn opt_float_json<T: Into<f64> + Copy>(v: Option<T>) -> Value {
    v.map(float_json).unwrap_or(Value::Null)
}

/// Render raw bytes (BINARY/BLOB columns) as uppercase hex, matching
/// MySQL's HEX() output for binary passes.
pub fn mysql_binary_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

pub fn mysql_value_to_json(row: &sqlx::mysql::MySqlRow, idx: usize, col_type: &str) -> Value {
    use sqlx::{Row, ValueRef};

    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return Value::Null;
        }
    }

    let upper = col_type.to_uppercase();
    match upper.as_str() {
        "DECIMAL" | "DEC" | "NUMERIC" | "FIXED" => {
            if let Ok(Some(v)) = row.try_get::<Option<rust_decimal::Decimal>, _>(idx) {
                return match v.to_string().parse::<f64>() {
                    Ok(n) if rust_decimal::Decimal::from_f64(n) == Some(v) => json!(n),
                    _ => json!(v.to_string()),
                };
            }
            return Value::Null;
        }
        "DATE" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDate>, _>(idx) {
                return json!(v.format("%Y-%m-%d").to_string());
            }
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
                return json!(v.format("%Y-%m-%d").to_string());
            }
        }
        "DATETIME" | "DATETIME2" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveDateTime>, _>(idx) {
                return json!(v.format("%Y-%m-%d %H:%M:%S%.3f").to_string());
            }
        }
        "TIMESTAMP" => {
            if let Ok(Some(v)) = row
                .try_get::<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>, _>(idx)
            {
                let local = v.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string());
            }
        }
        "TIME" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::chrono::NaiveTime>, _>(idx) {
                return json!(v.format("%H:%M:%S%.3f").to_string());
            }
        }
        "JSON" => {
            if let Ok(Some(v)) = row.try_get::<Option<sqlx::types::Json<Value>>, _>(idx) {
                return v.0;
            }
            if let Ok(Some(s)) = row.try_get::<Option<String>, _>(idx) {
                return serde_json::from_str(&s).unwrap_or(json!(s));
            }
            if let Ok(Some(b)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                let s = String::from_utf8_lossy(&b);
                return serde_json::from_str(&s).unwrap_or(json!(s.to_string()));
            }
            return Value::Null;
        }
        "BIGINT" => {
            if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
                let max_safe: i64 = 9_007_199_254_740_992;
                if v > -max_safe && v < max_safe {
                    return json!(v);
                } else {
                    return json!(v.to_string());
                }
            }
        }
        "BIGINT UNSIGNED" => {
            if let Ok(Some(v)) = row.try_get::<Option<u64>, _>(idx) {
                return json!(v.to_string());
            }
        }
        "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => {
            if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
                return json!(mysql_binary_to_hex(&v));
            }
            return Value::Null;
        }
        _ => {}
    }

    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
        return float_json(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(idx) {
        if let Some(parsed) = poste_core::sql_parser::parse_json_cell(&v) {
            return parsed;
        }
        if upper == "TIMESTAMP" || upper == "TIMESTAMP WITHOUT TIME ZONE" {
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&v, "%Y-%m-%d %H:%M:%S%.f") {
                let utc = dt.and_utc();
                let local = utc.with_timezone(&chrono::Local);
                return json!(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
            }
        }
        return json!(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        let s = String::from_utf8_lossy(&v);
        if let Some(parsed) = poste_core::sql_parser::parse_json_cell(&s) {
            return parsed;
        }
        return json!(s.to_string());
    }
    Value::Null
}

#[cfg(test)]
mod tests {
    use super::{float_json, mysql_binary_to_hex, opt_float_json};
    use serde_json::{json, Value};

    #[test]
    fn binary_to_hex_matches_mysql_hex() {
        assert_eq!(mysql_binary_to_hex(b""), "");
        assert_eq!(mysql_binary_to_hex(&[0x00, 0x0F, 0xA1]), "000FA1");
        assert_eq!(
            mysql_binary_to_hex(b"Hello, BINARY!"),
            "48656C6C6F2C2042494E41525921"
        );
        assert_eq!(
            mysql_binary_to_hex(&[0xFF, 0x00, 0x10, 0x1F, 0xA5, 0x5A]),
            "FF00101FA55A"
        );
    }

    /// The premise `float_json` exists for: serde_json has no spelling for a
    /// non-finite double, so it serializes to `null`. A postgres `float8`
    /// holding `Infinity` therefore reached the editor as NULL, and committing
    /// that row wrote NULL over the value.
    #[test]
    fn a_json_number_cannot_carry_infinity() {
        assert_eq!(json!(f64::INFINITY), Value::Null);
        assert_eq!(json!(f64::NEG_INFINITY), Value::Null);
        assert_eq!(json!(f64::NAN), Value::Null);
    }

    #[test]
    fn float_json_keeps_the_non_finite_values_as_their_engine_spelling() {
        assert_eq!(float_json(f64::INFINITY), json!("Infinity"));
        assert_eq!(float_json(f64::NEG_INFINITY), json!("-Infinity"));
        assert_eq!(float_json(f64::NAN), json!("NaN"));
        // f32 arrives through the same helper (FLOAT4 / FLOAT)
        assert_eq!(float_json(f32::INFINITY), json!("Infinity"));
        assert_eq!(float_json(f32::NAN), json!("NaN"));
    }

    #[test]
    fn float_json_leaves_finite_doubles_as_numbers() {
        assert_eq!(float_json(1.5_f64), json!(1.5));
        assert_eq!(float_json(0.0_f64), json!(0.0));
        // -0.0 is finite: it stays a number, and `= -0.0` matches in SQL
        assert_eq!(float_json(-0.0_f64), json!(-0.0));
        assert_eq!(float_json(f64::MAX), json!(f64::MAX));
    }

    #[test]
    fn opt_float_json_maps_none_to_null() {
        assert_eq!(opt_float_json(None::<f64>), Value::Null);
        assert_eq!(opt_float_json(Some(f64::NAN)), json!("NaN"));
    }
}

use serde_json::{json, Value};

pub(super) fn opt_json<T: serde::Serialize>(v: Option<T>) -> Value {
    v.map(|v| json!(v)).unwrap_or(Value::Null)
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
        json!(v.format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string())
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

//! Shared helpers for pulling typed values out of venue JSON.
//!
//! Every accessor names the field and the message it came from, so a feed that
//! changes shape produces an error that says exactly what moved rather than a
//! bare `Option::unwrap` panic in production.

use atas_core::{Price, Qty};
use serde_json::Value;

use crate::error::FeedError;

/// Fetch a field or report which one was missing.
pub fn field<'a>(v: &'a Value, name: &'static str, context: &'static str) -> Result<&'a Value, FeedError> {
    v.get(name).ok_or(FeedError::MissingField {
        field: name,
        context,
    })
}

/// Read a string field.
pub fn str_field<'a>(
    v: &'a Value,
    name: &'static str,
    context: &'static str,
) -> Result<&'a str, FeedError> {
    field(v, name, context)?
        .as_str()
        .ok_or_else(|| FeedError::BadField {
            field: name,
            context,
            value: v[name].to_string(),
        })
}

/// Read an integer field, accepting a JSON number or a numeric string.
///
/// Venues are inconsistent about this even between their own endpoints, so
/// accepting both is not laxity — it is the actual contract.
pub fn int_field(v: &Value, name: &'static str, context: &'static str) -> Result<i64, FeedError> {
    let raw = field(v, name, context)?;
    if let Some(n) = raw.as_i64() {
        return Ok(n);
    }
    if let Some(s) = raw.as_str() {
        if let Ok(n) = s.parse::<i64>() {
            return Ok(n);
        }
    }
    Err(FeedError::BadField {
        field: name,
        context,
        value: raw.to_string(),
    })
}

/// Read an unsigned integer field, accepting a number or numeric string.
pub fn uint_field(v: &Value, name: &'static str, context: &'static str) -> Result<u64, FeedError> {
    let raw = field(v, name, context)?;
    if let Some(n) = raw.as_u64() {
        return Ok(n);
    }
    if let Some(s) = raw.as_str() {
        if let Ok(n) = s.parse::<u64>() {
            return Ok(n);
        }
    }
    Err(FeedError::BadField {
        field: name,
        context,
        value: raw.to_string(),
    })
}

/// Read a decimal field into a [`Price`].
///
/// Venues send prices as strings precisely so clients do not route them
/// through a float, and this parses the string directly into fixed point
/// without ever touching `f64`.
pub fn price_field(
    v: &Value,
    name: &'static str,
    context: &'static str,
) -> Result<Price, FeedError> {
    let s = decimal_str(v, name, context)?;
    Price::parse(&s).map_err(|source| FeedError::BadNumber {
        field: name,
        context,
        source,
    })
}

/// Read a decimal field into a [`Qty`].
pub fn qty_field(v: &Value, name: &'static str, context: &'static str) -> Result<Qty, FeedError> {
    let s = decimal_str(v, name, context)?;
    Qty::parse(&s).map_err(|source| FeedError::BadNumber {
        field: name,
        context,
        source,
    })
}

/// A decimal field as text, whether the venue quoted it or not.
fn decimal_str(v: &Value, name: &'static str, context: &'static str) -> Result<String, FeedError> {
    let raw = field(v, name, context)?;
    match raw {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(FeedError::BadField {
            field: name,
            context,
            value: other.to_string(),
        }),
    }
}

/// Parse a `["price", "qty"]` book level pair.
pub fn level_pair(v: &Value, context: &'static str) -> Result<(Price, Qty), FeedError> {
    let arr = v.as_array().ok_or_else(|| FeedError::BadField {
        field: "level",
        context,
        value: v.to_string(),
    })?;
    if arr.len() < 2 {
        return Err(FeedError::BadField {
            field: "level",
            context,
            value: v.to_string(),
        });
    }

    let price = as_decimal_price(&arr[0], context)?;
    let qty = as_decimal_qty(&arr[1], context)?;
    Ok((price, qty))
}

fn as_decimal_price(v: &Value, context: &'static str) -> Result<Price, FeedError> {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => {
            return Err(FeedError::BadField {
                field: "level price",
                context,
                value: other.to_string(),
            })
        }
    };
    Price::parse(&text).map_err(|source| FeedError::BadNumber {
        field: "level price",
        context,
        source,
    })
}

fn as_decimal_qty(v: &Value, context: &'static str) -> Result<Qty, FeedError> {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => {
            return Err(FeedError::BadField {
                field: "level qty",
                context,
                value: other.to_string(),
            })
        }
    };
    Qty::parse(&text).map_err(|source| FeedError::BadNumber {
        field: "level qty",
        context,
        source,
    })
}

/// Parse an array of `["price", "qty"]` pairs.
pub fn level_array(
    v: &Value,
    name: &'static str,
    context: &'static str,
) -> Result<Vec<(Price, Qty)>, FeedError> {
    let Some(raw) = v.get(name) else {
        // An absent side simply means no updates for it in this message.
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let arr = raw.as_array().ok_or_else(|| FeedError::BadField {
        field: name,
        context,
        value: raw.to_string(),
    })?;
    arr.iter().map(|lvl| level_pair(lvl, context)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_numbers_whether_quoted_or_not() {
        let v = json!({ "a": 42, "b": "42" });
        assert_eq!(int_field(&v, "a", "t").unwrap(), 42);
        assert_eq!(int_field(&v, "b", "t").unwrap(), 42);
        assert_eq!(uint_field(&v, "a", "t").unwrap(), 42);
        assert_eq!(uint_field(&v, "b", "t").unwrap(), 42);
    }

    #[test]
    fn reads_decimals_without_touching_float() {
        let v = json!({ "p": "0.00000001", "q": "1.5" });
        assert_eq!(price_field(&v, "p", "t").unwrap(), Price::from_minor(1));
        assert_eq!(qty_field(&v, "q", "t").unwrap(), Qty::parse("1.5").unwrap());
    }

    #[test]
    fn names_the_missing_field() {
        let v = json!({ "a": 1 });
        let err = int_field(&v, "b", "ctx").unwrap_err();
        assert!(
            matches!(err, FeedError::MissingField { field: "b", context: "ctx" }),
            "{err:?}"
        );
    }

    #[test]
    fn rejects_unparseable_values() {
        let v = json!({ "a": "not-a-number", "p": "1.2.3" });
        assert!(matches!(
            int_field(&v, "a", "t"),
            Err(FeedError::BadField { .. })
        ));
        assert!(matches!(
            price_field(&v, "p", "t"),
            Err(FeedError::BadNumber { .. })
        ));
    }

    #[test]
    fn parses_book_levels() {
        let v = json!({ "b": [["100.50", "3"], ["100.25", "1.5"]] });
        let levels = level_array(&v, "b", "t").unwrap();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].0, Price::parse("100.50").unwrap());
        assert_eq!(levels[1].1, Qty::parse("1.5").unwrap());
    }

    #[test]
    fn an_absent_or_null_side_is_empty_not_an_error() {
        let v = json!({ "b": null });
        assert!(level_array(&v, "b", "t").unwrap().is_empty());
        assert!(level_array(&v, "a", "t").unwrap().is_empty());
    }

    #[test]
    fn rejects_a_malformed_level() {
        let v = json!({ "b": [["100.50"]] });
        assert!(matches!(
            level_array(&v, "b", "t"),
            Err(FeedError::BadField { .. })
        ));
    }
}

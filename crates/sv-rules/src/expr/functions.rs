//! Functions of the CEL subset.
//!
//! The evaluator runs with an empty `cel::Env`, so every named function resolves here. The standard
//! functions (`size`, `int`, `double`, `string`, `contains`, `startsWith`, `endsWith`, `matches`)
//! follow the cel-js 8.0.0 built-ins the browser uses; the SV functions are registered on both
//! sides from the same definitions (`cel-functions.ts`).

use std::sync::Arc;

use cel::extractors::This;
use cel::objects::Value;
use cel::{Context, ExecutionError};
use regex::Regex;
use time::OffsetDateTime;

use crate::decimal::compare_canonical_decimal;
use crate::limits::{MAX_TIMESTAMP, MIN_TIMESTAMP};
use crate::regex_subset::utf16_view;
use crate::text::{parse_iso_date, scalar_len};

type FnResult = Result<Value, ExecutionError>;

const SECONDS_PER_DAY: i64 = 86_400;

fn type_error(function: &str) -> ExecutionError {
    ExecutionError::function_error(function, "unsupported argument")
}

/// ECMAScript `String.prototype.trim` white space (`WhiteSpace` and `LineTerminator`).
pub const fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

fn size(This(v): This<Value>) -> FnResult {
    let n = match &v {
        Value::String(s) => scalar_len(s),
        Value::List(l) => l.len(),
        Value::Map(m) => m.map.len(),
        _ => return Err(type_error("size")),
    };
    i64::try_from(n).map(Value::Int).map_err(|_| type_error("size"))
}

/// cel-js `int(string)`: no surrounding white space, at most 20 UTF-16 units, no `0x`; then
/// ECMAScript `BigInt(string)` (empty is 0; `0b`, `0o`, `0X` prefixes; sign only for decimal) and
/// the int64 range.
fn int_from_string(s: &str) -> Option<i64> {
    if s.trim_matches(is_js_space) != s || s.encode_utf16().count() > 20 || s.contains("0x") {
        return None;
    }
    if s.is_empty() {
        return Some(0);
    }
    let (radix, digits) = match s.get(..2) {
        Some("0b" | "0B") => (2, s.get(2..)?),
        Some("0o" | "0O") => (8, s.get(2..)?),
        Some("0X") => (16, s.get(2..)?),
        _ => (10, s),
    };
    let (negative, digits) = if radix == 10 {
        match digits.as_bytes().first() {
            Some(b'-') => (true, digits.get(1..)?),
            Some(b'+') => (false, digits.get(1..)?),
            _ => (false, digits),
        }
    } else {
        (false, digits)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = i128::from_str_radix(digits, radix).ok()?;
    let value = if negative { -magnitude } else { magnitude };
    i64::try_from(value).ok()
}

fn int(This(v): This<Value>) -> FnResult {
    match &v {
        Value::Int(i) => Ok(Value::Int(*i)),
        Value::Float(f) => {
            let t = f.trunc();
            // i64::MAX as f64 rounds up to 2^63, so the upper bound is exclusive.
            #[allow(clippy::cast_precision_loss)]
            let in_range = t.is_finite() && t >= i64::MIN as f64 && t < i64::MAX as f64;
            if in_range {
                #[allow(clippy::cast_possible_truncation)]
                Ok(Value::Int(t as i64))
            } else {
                Err(type_error("int"))
            }
        }
        Value::String(s) => int_from_string(s).map(Value::Int).ok_or_else(|| type_error("int")),
        _ => Err(type_error("int")),
    }
}

/// cel-js `double(string)`: non-empty, no surrounding white space, the named infinities and NaN in
/// any case, otherwise ECMAScript `Number(string)`.
fn double_from_string(s: &str) -> Option<f64> {
    if s.is_empty() || s.trim_matches(is_js_space) != s {
        return None;
    }
    match s.to_lowercase().as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    let radix = match s.get(..2) {
        Some("0x" | "0X") => Some(16),
        Some("0b" | "0B") => Some(2),
        Some("0o" | "0O") => Some(8),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = s.get(2..)?;
        if digits.is_empty() {
            return None;
        }
        let mut value = 0.0_f64;
        for c in digits.chars() {
            value = value.mul_add(f64::from(radix), f64::from(c.to_digit(radix)?));
        }
        return Some(value);
    }
    // The named infinities are handled above; Rust also reads signed `nan`, which ECMAScript does not.
    s.parse::<f64>().ok().filter(|v| !v.is_nan())
}

fn double(This(v): This<Value>) -> FnResult {
    match &v {
        Value::Float(f) => Ok(Value::Float(*f)),
        #[allow(clippy::cast_precision_loss)]
        Value::Int(i) => Ok(Value::Float(*i as f64)),
        Value::String(s) => double_from_string(s)
            .map(Value::Float)
            .ok_or_else(|| type_error("double")),
        _ => Err(type_error("double")),
    }
}

/// ECMAScript `Number::toString` for a finite double, with cel-js spelling of the infinities.
pub fn format_double(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    if f == 0.0 {
        return "0".to_owned();
    }
    let sign = if f < 0.0 { "-" } else { "" };
    let sci = format!("{:e}", f.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exp: i64 = exp.parse().unwrap_or(0);
    let k = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let n = exp + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat(usize::try_from(n - k).unwrap_or(0)))
    } else if 0 < n && n <= 21 {
        let split = usize::try_from(n).unwrap_or(0);
        let (head, tail) = digits.split_at(split.min(digits.len()));
        format!("{head}.{tail}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(usize::try_from(-n).unwrap_or(0)))
    } else {
        let e = n - 1;
        let e_sign = if e >= 0 { "+" } else { "-" };
        let (first, rest) = digits.split_at(1.min(digits.len()));
        if rest.is_empty() {
            format!("{first}e{e_sign}{}", e.abs())
        } else {
            format!("{first}.{rest}e{e_sign}{}", e.abs())
        }
    };
    format!("{sign}{body}")
}

fn string(This(v): This<Value>) -> FnResult {
    let s = match &v {
        Value::String(s) => return Ok(Value::String(s.clone())),
        Value::Int(i) => i.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Float(f) => format_double(*f),
        _ => return Err(type_error("string")),
    };
    Ok(Value::String(Arc::new(s)))
}

fn string_pair<'a>(function: &str, this: &'a Value, arg: &'a Value) -> Result<(&'a str, &'a str), ExecutionError> {
    match (this, arg) {
        (Value::String(a), Value::String(b)) => Ok((a.as_str(), b.as_str())),
        _ => Err(type_error(function)),
    }
}

fn contains(This(this): This<Value>, arg: Value) -> FnResult {
    let (a, b) = string_pair("contains", &this, &arg)?;
    Ok(Value::Bool(a.contains(b)))
}

fn starts_with(This(this): This<Value>, arg: Value) -> FnResult {
    let (a, b) = string_pair("startsWith", &this, &arg)?;
    Ok(Value::Bool(a.starts_with(b)))
}

fn ends_with(This(this): This<Value>, arg: Value) -> FnResult {
    let (a, b) = string_pair("endsWith", &this, &arg)?;
    Ok(Value::Bool(a.ends_with(b)))
}

fn one_string(function: &str, v: &Value) -> Result<Arc<String>, ExecutionError> {
    match v {
        Value::String(s) => Ok(s.clone()),
        _ => Err(type_error(function)),
    }
}

fn lower(This(v): This<Value>) -> FnResult {
    Ok(Value::String(Arc::new(one_string("lower", &v)?.to_lowercase())))
}

fn upper(This(v): This<Value>) -> FnResult {
    Ok(Value::String(Arc::new(one_string("upper", &v)?.to_uppercase())))
}

fn trim(This(v): This<Value>) -> FnResult {
    let s = one_string("trim", &v)?;
    Ok(Value::String(Arc::new(s.trim_matches(is_js_space).to_owned())))
}

fn is_empty(This(v): This<Value>) -> FnResult {
    Ok(Value::Bool(match &v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::List(l) => l.is_empty(),
        Value::Map(m) => m.map.is_empty(),
        _ => false,
    }))
}

fn decimal_cmp(This(a): This<Value>, b: Value) -> FnResult {
    let (a, b) = string_pair("decimal_cmp", &a, &b)?;
    let ord = compare_canonical_decimal(a, b).ok_or_else(|| type_error("decimal_cmp"))?;
    Ok(Value::Int(ord as i64))
}

/// `YYYY-MM-DD` (years 0001..9999) at 00:00Z, in Unix seconds.
fn parse_date(s: &str) -> Option<i64> {
    parse_iso_date(s).map(|d| d.midnight().assume_utc().unix_timestamp())
}

fn date(This(v): This<Value>) -> FnResult {
    let s = one_string("date", &v)?;
    parse_date(&s).map(Value::Int).ok_or_else(|| type_error("date"))
}

fn days_between(This(a): This<Value>, b: Value) -> FnResult {
    match (&a, &b) {
        (Value::Int(a), Value::Int(b)) => b
            .checked_sub(*a)
            .map(|d| Value::Int(d / SECONDS_PER_DAY))
            .ok_or_else(|| type_error("days_between")),
        _ => Err(type_error("days_between")),
    }
}

fn utc(function: &str, v: &Value) -> Result<OffsetDateTime, ExecutionError> {
    match v {
        Value::Int(s) if (MIN_TIMESTAMP..=MAX_TIMESTAMP).contains(s) => {
            OffsetDateTime::from_unix_timestamp(*s).map_err(|_| type_error(function))
        }
        _ => Err(type_error(function)),
    }
}

fn get_full_year(This(v): This<Value>) -> FnResult {
    Ok(Value::Int(i64::from(utc("getFullYear", &v)?.year())))
}

fn get_month(This(v): This<Value>) -> FnResult {
    Ok(Value::Int(i64::from(u8::from(utc("getMonth", &v)?.month())) - 1))
}

fn get_date(This(v): This<Value>) -> FnResult {
    Ok(Value::Int(i64::from(utc("getDate", &v)?.day())))
}

fn get_day_of_week(This(v): This<Value>) -> FnResult {
    Ok(Value::Int(i64::from(
        utc("getDayOfWeek", &v)?.weekday().number_days_from_sunday(),
    )))
}

fn get_hours(This(v): This<Value>) -> FnResult {
    Ok(Value::Int(i64::from(utc("getHours", &v)?.hour())))
}

/// Registers every subset function. `patterns` holds the `matches` literals compiled at
/// [`crate::compile`] time.
pub fn register(ctx: &mut Context<'_>, patterns: Arc<Vec<(String, Regex)>>) {
    ctx.add_function("size", size);
    ctx.add_function("int", int);
    ctx.add_function("double", double);
    ctx.add_function("string", string);
    ctx.add_function("contains", contains);
    ctx.add_function("startsWith", starts_with);
    ctx.add_function("endsWith", ends_with);
    ctx.add_function("lower", lower);
    ctx.add_function("upper", upper);
    ctx.add_function("trim", trim);
    ctx.add_function("is_empty", is_empty);
    ctx.add_function("decimal_cmp", decimal_cmp);
    ctx.add_function("date", date);
    ctx.add_function("days_between", days_between);
    ctx.add_function("getFullYear", get_full_year);
    ctx.add_function("getMonth", get_month);
    ctx.add_function("getDate", get_date);
    ctx.add_function("getDayOfWeek", get_day_of_week);
    ctx.add_function("getHours", get_hours);
    ctx.add_function("matches", move |This(this): This<Value>, pattern: Value| -> FnResult {
        let (subject, pattern) = string_pair("matches", &this, &pattern)?;
        let re = patterns
            .iter()
            .find(|(p, _)| p == pattern)
            .map(|(_, re)| re)
            .ok_or_else(|| type_error("matches"))?;
        Ok(Value::Bool(re.is_match(&utf16_view(subject))))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_print_like_ecmascript() {
        assert_eq!(format_double(1.5), "1.5");
        assert_eq!(format_double(1.0), "1");
        assert_eq!(format_double(-0.0), "0");
        assert_eq!(format_double(1e21), "1e+21");
        assert_eq!(format_double(1e20), "100000000000000000000");
        assert_eq!(format_double(1.5e-7), "1.5e-7");
        assert_eq!(format_double(0.000_001), "0.000001");
        assert_eq!(format_double(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(format_double(-123.456), "-123.456");
    }

    #[test]
    fn int_parsing_follows_bigint() {
        assert_eq!(int_from_string(""), Some(0));
        assert_eq!(int_from_string("-42"), Some(-42));
        assert_eq!(int_from_string("0b101"), Some(5));
        assert_eq!(int_from_string("0X1f"), Some(31));
        assert_eq!(int_from_string("0x1f"), None);
        assert_eq!(int_from_string(" 1"), None);
        assert_eq!(int_from_string("1.5"), None);
        assert_eq!(int_from_string("9223372036854775808"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970-01-02"), Some(86_400));
        assert_eq!(parse_date("2026-02-30"), None);
        assert_eq!(parse_date("0000-01-01"), None);
    }
}

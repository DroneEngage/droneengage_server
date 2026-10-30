//! Helpers that keep the wire behaviour of the original Node.js server:
//! JavaScript-style coercion of JSON values (`String(x)`, `Number(x)`,
//! `parseInt`, `parseFloat`, `===`), `JSON.stringify` output (number
//! formatting, key order), wall-clock time and timestamps.

use serde_json::{Map, Value};
use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// `Date.now()`.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `Number.prototype.toString()` for a double.
pub fn js_number_fmt(p_value: f64) -> String {
    if p_value.is_nan() {
        return "NaN".into();
    }
    if p_value.is_infinite() {
        return if p_value > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if p_value == 0.0 {
        return "0".into();
    }
    // shortest round-trip digits and exponent, e.g. "1.2345e-7"
    let v_exp_form = format!("{:e}", p_value.abs());
    let (v_mantissa, v_exp) = v_exp_form.split_once('e').unwrap_or((&v_exp_form, "0"));
    let v_digits: String = v_mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let v_digits = v_digits.trim_end_matches('0');
    let v_digits = if v_digits.is_empty() { "0" } else { v_digits };
    let k = v_digits.len() as i64;
    let n = v_exp.parse::<i64>().unwrap_or(0) + 1;
    let mut v_out = String::new();
    if p_value < 0.0 {
        v_out.push('-');
    }
    if k <= n && n <= 21 {
        v_out.push_str(v_digits);
        v_out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        v_out.push_str(&v_digits[..n as usize]);
        v_out.push('.');
        v_out.push_str(&v_digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        v_out.push_str("0.");
        v_out.extend(std::iter::repeat_n('0', (-n) as usize));
        v_out.push_str(v_digits);
    } else {
        v_out.push_str(&v_digits[..1]);
        if k > 1 {
            v_out.push('.');
            v_out.push_str(&v_digits[1..]);
        }
        let _ = write!(v_out, "e{}{}", if n > 0 { "+" } else { "-" }, (n - 1).abs());
    }
    v_out
}

/// The JavaScript number a JSON number becomes (`JSON.parse` yields doubles).
pub fn json_number_f64(p_value: &serde_json::Number) -> f64 {
    p_value.as_f64().unwrap_or(f64::NAN)
}

fn number_fmt(p_value: &serde_json::Number) -> String {
    if let Some(i) = p_value.as_i64() {
        if i.unsigned_abs() <= (1u64 << 53) {
            return i.to_string();
        }
    }
    if let Some(u) = p_value.as_u64() {
        if u <= (1u64 << 53) {
            return u.to_string();
        }
    }
    js_number_fmt(json_number_f64(p_value))
}

/// `String(p_value)` for a JSON value.
pub fn js_to_string(p_value: &Value) -> String {
    match p_value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_fmt(n),
        Value::String(s) => s.clone(),
        Value::Array(a) => {
            a.iter().map(|v| if v.is_null() { String::new() } else { js_to_string(v) }).collect::<Vec<_>>().join(",")
        }
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `String(x)` where a missing value is JavaScript's `undefined`.
pub fn js_key(p_value: Option<&Value>) -> String {
    match p_value {
        None => "undefined".to_string(),
        Some(v) => js_to_string(v),
    }
}

/// Template-literal interpolation `${x}` (same as `String(x)`).
pub fn js_interp(p_value: Option<&Value>) -> String {
    js_key(p_value)
}

/// `JSON.stringify(p_value)`: no whitespace, JavaScript number formatting
/// (integers above 2^53 and doubles as a JS engine prints them; NaN/Infinity
/// can't occur in parsed JSON), keys in insertion order.
pub fn js_stringify(p_value: &Value) -> String {
    let mut v_out = String::new();
    stringify_into(p_value, &mut v_out);
    v_out
}

fn stringify_into(p_value: &Value, p_out: &mut String) {
    match p_value {
        Value::Null => p_out.push_str("null"),
        Value::Bool(b) => p_out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => p_out.push_str(&number_fmt(n)),
        Value::String(s) => p_out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Value::Array(a) => {
            p_out.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    p_out.push(',');
                }
                if is_undefined(v) {
                    p_out.push_str("null");
                } else {
                    stringify_into(v, p_out);
                }
            }
            p_out.push(']');
        }
        Value::Object(m) => {
            p_out.push('{');
            let mut v_first = true;
            for (k, v) in m.iter() {
                if is_undefined(v) {
                    continue;
                }
                if !std::mem::take(&mut v_first) {
                    p_out.push(',');
                }
                p_out.push_str(&serde_json::to_string(k).unwrap_or_default());
                p_out.push(':');
                stringify_into(v, p_out);
            }
            p_out.push('}');
        }
    }
}

/// `JSON.parse(text)`; None where JavaScript would throw.
pub fn js_parse(p_text: &str) -> Option<Value> {
    serde_json::from_str(p_text).ok()
}

/// `JSON.parse(buffer)`: the bytes are decoded as UTF-8 (invalid sequences
/// become U+FFFD, like Buffer.toString()).
pub fn js_parse_bytes(p_bytes: &[u8]) -> Option<Value> {
    match std::str::from_utf8(p_bytes) {
        Ok(s) => js_parse(s),
        Err(_) => js_parse(&String::from_utf8_lossy(p_bytes)),
    }
}

/// `Number(p_value)`; a missing value is `undefined` (NaN).
pub fn js_number(p_value: Option<&Value>) -> f64 {
    match p_value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(n)) => json_number_f64(n),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                0.0
            } else if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                i64::from_str_radix(h, 16).map(|v| v as f64).unwrap_or(f64::NAN)
            } else if t == "Infinity" || t == "+Infinity" {
                f64::INFINITY
            } else if t == "-Infinity" {
                f64::NEG_INFINITY
            } else if t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')) {
                t.parse::<f64>().unwrap_or(f64::NAN)
            } else {
                f64::NAN
            }
        }
        Some(Value::Array(a)) => match a.len() {
            0 => 0.0,
            1 => js_number(Some(&a[0])),
            _ => f64::NAN,
        },
        Some(Value::Object(_)) => f64::NAN,
    }
}

/// `parseFloat(p_value)`.
pub fn js_parse_float(p_value: Option<&Value>) -> f64 {
    let v_text = js_key(p_value);
    let t = v_text.trim_start();
    // longest prefix that is a decimal literal
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if t[i..].starts_with("Infinity") {
        return if b.first() == Some(&b'-') { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    let v_start_digits = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == v_start_digits || (i == v_start_digits + 1 && b[v_start_digits] == b'.') {
        return f64::NAN;
    }
    let mut v_end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let v_exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > v_exp_start {
            v_end = j;
        }
    }
    t[..v_end].parse::<f64>().unwrap_or(f64::NAN)
}

/// `parseInt(p_value, 10)`; None for NaN.
pub fn js_parse_int(p_value: Option<&Value>) -> Option<i64> {
    let v_text = match p_value? {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Some(i);
            }
            let f = n.as_f64()?;
            if !f.is_finite() {
                return None;
            }
            if f.abs() < 1e21 {
                return Some(f.trunc() as i64);
            }
            js_number_fmt(f)
        }
        other => js_to_string(other),
    };
    let t = v_text.trim_start();
    let (v_neg, v_digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let v_end = v_digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(v_digits.len());
    if v_end == 0 {
        return None;
    }
    let v: i64 = v_digits[..v_end].parse().ok()?;
    Some(if v_neg { -v } else { v })
}

/// `p_a === p_b` for JSON values (objects/arrays never compare equal here:
/// distinct parses are distinct objects).
pub fn js_strict_eq(p_a: Option<&Value>, p_b: Option<&Value>) -> bool {
    match (p_a, p_b) {
        (None, None) => true,
        (Some(Value::Number(x)), Some(Value::Number(y))) => json_number_f64(x) == json_number_f64(y),
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Null), Some(Value::Null)) => true,
        _ => false,
    }
}

/// JavaScript truthiness.
pub fn js_truthy(p_value: Option<&Value>) -> bool {
    match p_value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => {
            let f = json_number_f64(n);
            f != 0.0 && !f.is_nan()
        }
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// `x == null` (null or undefined).
pub fn js_nullish(p_value: Option<&Value>) -> bool {
    matches!(p_value, None | Some(Value::Null))
}

/// `p_value === true`.
pub fn is_true(p_value: Option<&Value>) -> bool {
    matches!(p_value, Some(Value::Bool(true)))
}

/// `x.length` for strings (UTF-16 code units) and arrays; None (undefined)
/// for everything else.
pub fn js_length(p_value: Option<&Value>) -> Option<usize> {
    match p_value? {
        Value::String(s) => Some(s.encode_utf16().count()),
        Value::Array(a) => Some(a.len()),
        _ => None,
    }
}

/// `(x == null) || (x.length == 0)` — the "missing or empty" test used by the
/// command handlers. Throws (None) where JavaScript would throw.
pub fn js_missing_or_empty(p_value: Option<&Value>) -> bool {
    if js_nullish(p_value) {
        return true;
    }
    // `.length == 0`: undefined == 0 is false; "" and [] have length 0;
    // `[] == 0` and `"" == 0` are true via length anyway
    js_length(p_value) == Some(0)
}

/// Field of an object; None for non-objects and undefined properties.
pub fn field<'a>(p_value: &'a Value, p_key: &str) -> Option<&'a Value> {
    match p_value {
        Value::Object(m) => m.get(p_key).filter(|v| !is_undefined(v)),
        _ => None,
    }
}

/// Field of an object as an owned value.
pub fn field_cloned(p_value: &Value, p_key: &str) -> Option<Value> {
    field(p_value, p_key).cloned()
}

/// Sets `obj[key] = value` keeping the key's position when it exists
/// (JavaScript property order). No-op for non-objects.
pub fn set_field(p_obj: &mut Value, p_key: &str, p_value: Value) {
    if let Value::Object(m) = p_obj {
        m.insert(p_key.to_string(), p_value);
    }
}

/// An object property set to `undefined`: it keeps its place in the object
/// (a later assignment does not move it) and JSON.stringify omits it.
const UNDEFINED: &str = "\u{0}\u{1}undefined\u{1}\u{0}";

fn is_undefined(p_value: &Value) -> bool {
    matches!(p_value, Value::String(s) if s == UNDEFINED)
}

/// Sets `obj[key] = value` where value may be `undefined`.
pub fn set_field_opt(p_obj: &mut Value, p_key: &str, p_value: Option<Value>) {
    if let Value::Object(m) = p_obj {
        m.insert(p_key.to_string(), p_value.unwrap_or_else(|| Value::String(UNDEFINED.to_string())));
    }
}

/// A new empty JSON object.
pub fn obj() -> Value {
    Value::Object(Map::new())
}

/// `new Date(p_ms).toISOString()`.
pub fn iso_time(p_ms: i64) -> String {
    let (y, mo, d, h, mi, s, ms) = civil(p_ms);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, mo, d, h, mi, s, ms)
}

/// UTC calendar fields of a millisecond timestamp.
pub fn civil(p_ms: i64) -> (i64, i64, i64, i64, i64, i64, i64) {
    let secs = p_ms.div_euclid(1000);
    let ms = p_ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let sod = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, sod / 3600, (sod % 3600) / 60, sod % 60, ms)
}

/// Random UUID v4 string (uuidv4()).
pub fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn number_formatting_matches_javascript() {
        let cases: [(f64, &str); 16] = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (1.5, "1.5"),
            (-2.25, "-2.25"),
            (100.0, "100"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (0.000001, "0.000001"),
            (0.0000012, "0.0000012"),
            (31.2345678 * 1.0, "31.2345678"),
            (f64::NAN, "NaN"),
        ];
        for (v, s) in cases {
            assert_eq!(js_number_fmt(v), s, "{}", v);
        }
        assert_eq!(js_number_fmt(-1e-10), "-1e-10");
        assert_eq!(js_number_fmt(1.0e300), "1e+300");
    }

    #[test]
    fn stringify_matches_javascript() {
        let v: Value = serde_json::from_str(
            r#"{"b":1.0,"a":[1e2,2.50,"x\u0001\n"],"c":{"d":null,"e":true},"n":12345678901234567890}"#,
        )
        .unwrap();
        assert_eq!(
            js_stringify(&v),
            r#"{"b":1,"a":[100,2.5,"x\u0001\n"],"c":{"d":null,"e":true},"n":12345678901234567000}"#
        );
        let mut m = json!({"ty": "s", "mt": 9005, "ms": {}});
        set_field(&mut m, "ty", json!("i"));
        set_field(&mut m, "p", json!(4294967295u64));
        assert_eq!(js_stringify(&m), r#"{"ty":"i","mt":9005,"ms":{},"p":4294967295}"#);
        // obj.mt = undefined keeps mt's place for a later assignment
        set_field_opt(&mut m, "mt", None);
        set_field_opt(&mut m, "x", None);
        assert_eq!(js_stringify(&m), r#"{"ty":"i","ms":{},"p":4294967295}"#);
        assert!(field(&m, "mt").is_none());
        set_field(&mut m, "mt", json!(1));
        assert_eq!(js_stringify(&m), r#"{"ty":"i","mt":1,"ms":{},"p":4294967295}"#);
    }

    #[test]
    fn coercions() {
        assert_eq!(js_to_string(&json!(5.0)), "5");
        assert_eq!(js_key(None), "undefined");
        assert_eq!(js_parse_int(Some(&json!("40001abc"))), Some(40001));
        assert!(js_parse_float(Some(&json!("abc"))).is_nan());
        assert_eq!(js_parse_float(Some(&json!(" 31.5xyz"))), 31.5);
        assert_eq!(js_parse_float(Some(&json!("1e3"))), 1000.0);
        assert!(js_strict_eq(Some(&json!(1)), Some(&json!(1.0))));
        assert!(!js_strict_eq(Some(&json!("1")), Some(&json!(1))));
        assert!(js_missing_or_empty(Some(&json!(""))));
        assert!(js_missing_or_empty(None));
        assert!(!js_missing_or_empty(Some(&json!(5))));
        assert_eq!(js_length(Some(&json!("é😀"))), Some(3));
        assert_eq!(iso_time(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(js_number(Some(&json!("19001"))), 19001.0);
    }
}

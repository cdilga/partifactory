//! JSON Canonicalization Scheme (RFC 8785) for the values manifests contain: objects with keys
//! sorted by UTF-16 code units, no whitespace, strings escaped as ECMAScript `JSON.stringify`
//! does, integers only (manifests never hold floats; a float is refused rather than risk an
//! ECMAScript number-formatting mismatch).

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("JCS: non-integer numbers are not allowed in manifests")]
pub struct FloatNotAllowed;

pub fn canonicalize(v: &Value) -> Result<String, FloatNotAllowed> {
    let mut out = String::new();
    write(v, &mut out)?;
    Ok(out)
}

fn write(v: &Value, out: &mut String) -> Result<(), FloatNotAllowed> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                return Err(FloatNotAllowed);
            }
        }
        Value::String(s) => string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out)?;
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(k, out);
                out.push(':');
                write(&m[*k], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rfc8785_examples() {
        // Key order by UTF-16 code units (RFC 8785 §3.2.3's example, integers only).
        let v = json!({"\u{20ac}": "Euro Sign", "\r": "Carriage Return", "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
                       "1": "One", "\u{1f600}": "Emoji: Grinning Face", "\u{80}": "Control", "\u{f6}": "Latin Small Letter O With Diaeresis"});
        assert_eq!(
            canonicalize(&v).unwrap(),
            "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{80}\":\"Control\",\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",\
             \"\u{20ac}\":\"Euro Sign\",\"\u{1f600}\":\"Emoji: Grinning Face\",\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
        );
        assert_eq!(
            canonicalize(&json!({"b": [1, null, true], "a": "\u{1}x/"})).unwrap(),
            "{\"a\":\"\\u0001x/\",\"b\":[1,null,true]}"
        );
        assert!(canonicalize(&json!({"f": 1.5})).is_err());
    }
}

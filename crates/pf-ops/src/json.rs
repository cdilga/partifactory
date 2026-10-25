//! Decode before typed dispatch without silently losing duplicate object keys.
use pf_model::PfError;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Strict, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = a.next_element::<Strict>()? {
                    out.push(v.0);
                }
                Ok(Strict(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Strict, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some((key, value)) = m.next_entry::<String, Strict>()? {
                    if out.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!("duplicate field {key:?}")));
                    }
                    out.insert(key, value.0);
                }
                Ok(Strict(Value::Object(out)))
            }
        }
        d.deserialize_any(V)
    }
}
pub fn decode(bytes: &[u8]) -> Result<Value, PfError> {
    serde_json::from_slice::<Strict>(bytes)
        .map(|v| v.0)
        .map_err(|e| PfError::invalid("body", e.to_string()))
}
#[cfg(test)]
mod tests {
    #[test]
    fn duplicate_paths_never_disappear() {
        assert!(super::decode(br#"{"files":{"index.html":"a","index.html":"b"}}"#).is_err());
        assert!(super::decode(br#"{"value":[1,true,null,1.5,"x"]}"#).is_ok());
    }
}

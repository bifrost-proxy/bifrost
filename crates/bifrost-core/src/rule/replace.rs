use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

struct OrderedReplaceObject(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for OrderedReplaceObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OrderedReplaceObjectVisitor;

        impl<'de> Visitor<'de> for OrderedReplaceObjectVisitor {
            type Value = OrderedReplaceObject;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object containing replacement pairs")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut pairs: Vec<(String, Value)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Value>()? {
                    if let Some((_, existing_value)) = pairs
                        .iter_mut()
                        .find(|(existing_key, _)| existing_key == &key)
                    {
                        *existing_value = value;
                    } else {
                        pairs.push((key, value));
                    }
                }
                Ok(OrderedReplaceObject(pairs))
            }
        }

        deserializer.deserialize_map(OrderedReplaceObjectVisitor)
    }
}

/// Parses a strict JSON object used as a Whistle-compatible replace table.
///
/// Returns `None` for non-object or malformed JSON so callers can preserve
/// their legacy inline parsing behavior. Object entry order is preserved;
/// duplicate keys use the last value without changing the first position.
pub fn parse_json_replace_pairs(value: &str) -> Option<Vec<(String, String)>> {
    let mut deserializer = serde_json::Deserializer::from_str(value.trim());
    let OrderedReplaceObject(entries) =
        OrderedReplaceObject::deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;

    let mut pairs = Vec::with_capacity(entries.len());
    for (key, value) in entries {
        let replacement = match value {
            Value::String(value) => value,
            value => serde_json::to_string(&value).ok()?,
        };
        pairs.push((key, replacement));
    }
    Some(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_supplied_json_replace_object_with_escapes_in_order() {
        let value = r#"{
            ".doupay.com\"": ".nodoupay.com\"",
            "\"inf.baohuaxia.com\"": "\"inf.nobaohuaxia.com\"",
            "\"pf.baohuaxia.com\"": "\"pf.nobaohuaxia.com\""
        }"#;

        assert_eq!(
            parse_json_replace_pairs(value),
            Some(vec![
                (r#".doupay.com""#.into(), r#".nodoupay.com""#.into()),
                (
                    r#""inf.baohuaxia.com""#.into(),
                    r#""inf.nobaohuaxia.com""#.into(),
                ),
                (
                    r#""pf.baohuaxia.com""#.into(),
                    r#""pf.nobaohuaxia.com""#.into(),
                ),
            ])
        );
    }

    #[test]
    fn preserves_first_position_and_last_value_for_duplicate_keys() {
        assert_eq!(
            parse_json_replace_pairs(r#"{"first":"1","second":"2","first":"3"}"#),
            Some(vec![
                ("first".into(), "3".into()),
                ("second".into(), "2".into()),
            ])
        );
    }

    #[test]
    fn serializes_non_string_json_values_compactly() {
        assert_eq!(
            parse_json_replace_pairs(
                r#"{"number":12,"bool":true,"null":null,"array":[1,"x"],"object":{"x":1}}"#,
            ),
            Some(vec![
                ("number".into(), "12".into()),
                ("bool".into(), "true".into()),
                ("null".into(), "null".into()),
                ("array".into(), r#"[1,"x"]"#.into()),
                ("object".into(), r#"{"x":1}"#.into()),
            ])
        );
    }

    #[test]
    fn rejects_non_object_or_malformed_json_for_legacy_fallback() {
        assert_eq!(parse_json_replace_pairs(r#"["a","b"]"#), None);
        assert_eq!(parse_json_replace_pairs(r#"{"a":}"#), None);
        assert_eq!(parse_json_replace_pairs("old=new"), None);
    }

    #[test]
    fn accepts_empty_json_object_without_creating_a_delete_rule() {
        assert_eq!(parse_json_replace_pairs("{}"), Some(Vec::new()));
    }
}

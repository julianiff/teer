//! teer's answer schemas, adjusted for the services that hold a model to a
//! schema while it writes. They take a subset of JSON Schema: Anthropic's
//! structured outputs want every object closed and no length, count or
//! number limits; OpenAI's strict mode (Codex, `--output-schema`) wants every
//! property required. The limits teer sets stay in words in the description,
//! where the model still reads them; teer checks the answers itself anyway.

use serde_json::{Map, Value};

/// Limits Anthropic's structured outputs reject, and how they read in words.
const LIMITS: &[(&str, &str)] = &[
    ("maxLength", "at most {} characters"),
    ("minLength", "at least {} characters"),
    ("maxItems", "at most {} items"),
    ("minItems", "at least {} items"),
    ("maximum", "at most {}"),
    ("minimum", "at least {}"),
    ("exclusiveMaximum", "below {}"),
    ("exclusiveMinimum", "above {}"),
    ("multipleOf", "a multiple of {}"),
    ("pattern", "matching {}"),
    ("uniqueItems", "unique items: {}"),
];

/// For Anthropic's structured outputs (`output_config.format`): objects
/// closed, unsupported limits moved into the description. `minItems` of 0
/// or 1 is supported and stays.
pub fn for_anthropic(schema: &Value) -> Value {
    let mut s = schema.clone();
    walk(&mut s, &mut |o| {
        close(o);
        let mut words = Vec::new();
        for (key, text) in LIMITS {
            let small = *key == "minItems" && o.get(*key).and_then(Value::as_u64) <= Some(1);
            if small {
                continue;
            }
            if let Some(v) = o.remove(*key) {
                let v = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                words.push(text.replace("{}", &v));
            }
        }
        describe(o, &words);
    });
    s
}

/// For OpenAI's strict mode: objects closed, and every property required;
/// the ones teer leaves optional may be `null` instead (see [`drop_nulls`]).
pub fn for_openai(schema: &Value) -> Value {
    let mut s = schema.clone();
    walk(&mut s, &mut |o| {
        close(o);
        let required: Vec<String> = match o.get("required") {
            Some(Value::Array(r)) => r
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => Vec::new(),
        };
        let Some(Value::Object(props)) = o.get_mut("properties") else {
            return;
        };
        let mut all = Vec::new();
        for (name, p) in props.iter_mut() {
            if !required.contains(name) {
                *p = serde_json::json!({ "anyOf": [p.take(), { "type": "null" }] });
            }
            all.push(Value::String(name.clone()));
        }
        o.insert("required".into(), Value::Array(all));
    });
    s
}

/// Optional fields a strict model filled with `null` are left out again,
/// as teer's readers expect.
pub fn drop_nulls(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, drop_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(drop_nulls).collect()),
        v => v,
    }
}

/// Call `f` on every schema object in `v`, innermost first.
fn walk(v: &mut Value, f: &mut impl FnMut(&mut Map<String, Value>)) {
    match v {
        Value::Object(o) => {
            for (k, child) in o.iter_mut() {
                // Property names are not schemas; their values are.
                if k == "properties" {
                    if let Value::Object(props) = child {
                        for p in props.values_mut() {
                            walk(p, f);
                        }
                    }
                } else if k != "enum" && k != "const" && k != "required" {
                    walk(child, f);
                }
            }
            if o.contains_key("type") || o.contains_key("properties") || o.contains_key("anyOf") {
                f(o);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| walk(x, f)),
        _ => {}
    }
}

fn close(o: &mut Map<String, Value>) {
    let object =
        o.get("type").and_then(Value::as_str) == Some("object") || o.contains_key("properties");
    if object {
        o.insert("additionalProperties".into(), Value::Bool(false));
    }
}

fn describe(o: &mut Map<String, Value>, words: &[String]) {
    if words.is_empty() {
        return;
    }
    let add = words.join(", ");
    let text = match o.get("description").and_then(Value::as_str) {
        Some(d) => format!("{d} ({add})"),
        None => format!("{}{}", add[..1].to_uppercase(), &add[1..]),
    };
    o.insert("description".into(), Value::String(text));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "type": "object",
            "required": ["summary", "items"],
            "properties": {
                "summary": { "type": "string", "maxLength": 90 },
                "items": {
                    "type": "array", "minItems": 1, "maxItems": 3,
                    "items": {
                        "type": "object",
                        "properties": { "n": { "type": "integer", "minimum": 1, "description": "Line" } },
                        "required": ["n"]
                    }
                },
                "note": { "type": "string" }
            }
        })
    }

    #[test]
    fn anthropic_closes_objects_and_puts_limits_into_words() {
        let s = for_anthropic(&sample());
        assert_eq!(s["additionalProperties"], json!(false));
        assert_eq!(
            s["properties"]["summary"],
            json!({ "type": "string", "description": "At most 90 characters" })
        );
        let items = &s["properties"]["items"];
        assert_eq!(items["minItems"], json!(1));
        assert!(items.get("maxItems").is_none());
        assert_eq!(items["description"], json!("At most 3 items"));
        let inner = &items["items"];
        assert_eq!(inner["additionalProperties"], json!(false));
        assert_eq!(
            inner["properties"]["n"]["description"],
            json!("Line (at least 1)")
        );
        // Optional stays optional.
        assert_eq!(s["required"], json!(["summary", "items"]));
    }

    #[test]
    fn openai_requires_everything_and_lets_the_optional_be_null() {
        let s = for_openai(&sample());
        let mut req: Vec<&str> = s["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        req.sort();
        assert_eq!(req, ["items", "note", "summary"]);
        assert_eq!(
            s["properties"]["note"],
            json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
        );
        assert_eq!(s["properties"]["summary"]["maxLength"], json!(90));
        assert_eq!(
            s["properties"]["items"]["items"]["additionalProperties"],
            json!(false)
        );
    }

    #[test]
    fn nulls_are_dropped_at_every_depth() {
        let v = json!({ "a": null, "b": [{ "c": null, "d": 1 }], "e": "x" });
        assert_eq!(drop_nulls(v), json!({ "b": [{ "d": 1 }], "e": "x" }));
    }

    #[test]
    fn enum_values_and_property_names_are_left_alone() {
        let s = json!({
            "type": "object",
            "properties": {
                "type": { "type": "string", "enum": ["object", "maxLength"] },
                "properties": { "type": "string" }
            }
        });
        let a = for_anthropic(&s);
        assert_eq!(
            a["properties"]["type"]["enum"],
            json!(["object", "maxLength"])
        );
        assert_eq!(a["properties"]["properties"], json!({ "type": "string" }));
    }
}

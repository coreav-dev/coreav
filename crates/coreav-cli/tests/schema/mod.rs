//! A small JSON Schema validator for the tests of the `coreav` binary.
//!
//! Supports the subset the schemas under `docs/schema/` use: type, const,
//! enum, minimum, properties, required, additionalProperties: false, items,
//! anyOf and local $ref.

use serde_json::Value;

fn type_ok(value: &Value, name: &str) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        other => panic!("schema type {other} not supported"),
    }
}

pub fn validate(value: &Value, schema: &Value, root: &Value, at: &str) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.strip_prefix("#/$defs/").unwrap();
        return validate(value, &root["$defs"][name], root, at);
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        let errors: Vec<String> = options
            .iter()
            .filter_map(|s| validate(value, s, root, at).err())
            .collect();
        if errors.len() == options.len() {
            return Err(format!("{at}: no anyOf branch matches: {errors:?}"));
        }
    }
    if let Some(expected) = schema.get("const") {
        if value != expected {
            return Err(format!("{at}: {value} is not {expected}"));
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{at}: {value} not in {allowed:?}"));
        }
    }
    if let Some(types) = schema.get("type") {
        let ok = match types {
            Value::String(name) => type_ok(value, name),
            Value::Array(names) => names.iter().any(|n| type_ok(value, n.as_str().unwrap())),
            _ => panic!("bad type in schema"),
        };
        if !ok {
            return Err(format!("{at}: {value} is not of type {types}"));
        }
    }
    if let (Some(minimum), Some(number)) = (schema.get("minimum"), value.as_f64()) {
        if number < minimum.as_f64().unwrap() {
            return Err(format!("{at}: {value} below {minimum}"));
        }
    }
    if let Some(text) = value.as_str() {
        let chars = text.chars().count() as u64;
        if schema
            .get("minLength")
            .is_some_and(|min| chars < min.as_u64().unwrap())
            || schema
                .get("maxLength")
                .is_some_and(|max| chars > max.as_u64().unwrap())
        {
            return Err(format!("{at}: {value} has the wrong length"));
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        for key in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = key.as_str().unwrap();
            if !object.contains_key(key) {
                return Err(format!("{at}: missing key {key}"));
            }
        }
        for (key, item) in object {
            match properties.and_then(|p| p.get(key)) {
                Some(sub) => validate(item, sub, root, &format!("{at}.{key}"))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{at}: undocumented key {key}"));
                }
                None => {}
            }
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, item) in array.iter().enumerate() {
            validate(item, items, root, &format!("{at}[{i}]"))?;
        }
    }
    Ok(())
}

/// Panics with the first violation of `schema` (JSON text) by `document`.
pub fn assert_matches(document: &Value, schema: &str) {
    let root: Value = serde_json::from_str(schema).unwrap();
    if let Err(error) = validate(document, &root, &root, "$") {
        panic!("schema violation: {error}\n{document:#}");
    }
}

//! A small JSON Schema checker for tests (no validator crate): enough of draft 2020-12 for the
//! protocol schema's `live_frame`, the write API schema and the actions' own schemas. Unknown
//! keywords make the check fail loudly instead of passing silently.
//!
//! The two schemas of the connector kit this crate's formats follow are embedded as
//! [`WRITE_API_SCHEMA`] and [`PROTOCOL_SCHEMA`] (copies of the kit's files, release 0.6.0 for the write API schema).

use serde_json::Value;

/// The write API schema (`$id` `https://pitwall.local/write-api/v1.schema.json`).
pub const WRITE_API_SCHEMA: &str = include_str!("../tests/fixtures/write-api.schema.json");
/// The connector protocol schema (`$id` `https://pitwall.local/connector-protocol/v1.schema.json`).
pub const PROTOCOL_SCHEMA: &str = include_str!("../tests/fixtures/connector-protocol.schema.json");

const IGNORED: &[&str] = &[
    "description",
    "title",
    "$comment",
    "$id",
    "$schema",
    "$defs",
    "format",
    "pattern",
    "examples",
    "default",
    "example",
    "contentEncoding",
];

/// Checks `v` against `$defs/<name>` of the first document of `docs`; returns the first errors.
pub fn definition(docs: &[Value], name: &str, v: &Value) -> Result<(), String> {
    let schema = &docs[0]["$defs"][name];
    assert!(schema.is_object(), "no $defs/{name}");
    check_schema(docs, &docs[0], schema, v)
}

/// Checks `v` against `schema`; refs resolve inside `root` or, with a base URL, in the document
/// of `docs` with that `$id`.
pub fn check_schema(docs: &[Value], root: &Value, schema: &Value, v: &Value) -> Result<(), String> {
    let mut errs = Vec::new();
    check(docs, root, schema, v, "", &mut errs);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} error(s): {}",
            errs.len(),
            errs[..errs.len().min(5)].join("; ")
        ))
    }
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "number" => v.is_number(),
        "integer" => {
            v.as_i64().is_some()
                || v.as_u64().is_some()
                || v.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        _ => false,
    }
}

fn check(
    docs: &[Value],
    root: &Value,
    schema: &Value,
    v: &Value,
    path: &str,
    errs: &mut Vec<String>,
) {
    let Some(s) = schema.as_object() else {
        if schema != &Value::Bool(true) {
            errs.push(format!("{path}: schema {schema} not supported"));
        }
        return;
    };
    for (key, rule) in s {
        match key.as_str() {
            k if IGNORED.contains(&k) => {}
            "$ref" => {
                let (base, frag) = rule
                    .as_str()
                    .and_then(|r| r.split_once('#'))
                    .unwrap_or(("", ""));
                let doc = if base.is_empty() {
                    Some(root)
                } else {
                    docs.iter().find(|d| d["$id"].as_str() == Some(base))
                };
                match doc.and_then(|d| d.pointer(frag).map(|t| (d, t))) {
                    Some((d, t)) => check(docs, d, t, v, path, errs),
                    None => errs.push(format!("{path}: unresolved {rule}")),
                }
            }
            "if" => {
                let branch = if check_schema(docs, root, rule, v).is_ok() {
                    "then"
                } else {
                    "else"
                };
                if let Some(b) = s.get(branch) {
                    check(docs, root, b, v, path, errs);
                }
            }
            "then" | "else" => {}
            "not" => {
                if check_schema(docs, root, rule, v).is_ok() {
                    errs.push(format!("{path}: {v} matches a forbidden schema"));
                }
            }
            "type" => {
                let ok = match rule {
                    Value::String(t) => type_ok(t, v),
                    Value::Array(ts) => {
                        ts.iter().any(|t| t.as_str().is_some_and(|t| type_ok(t, v)))
                    }
                    _ => false,
                };
                if !ok {
                    errs.push(format!("{path}: {v} is not of type {rule}"));
                }
            }
            "enum" => {
                if !rule.as_array().is_some_and(|a| a.contains(v)) {
                    errs.push(format!("{path}: {v} not in {rule}"));
                }
            }
            "const" => {
                if rule != v {
                    errs.push(format!("{path}: {v} is not {rule}"));
                }
            }
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => {
                if let (Some(x), Some(b)) = (v.as_f64(), rule.as_f64()) {
                    let ok = match key.as_str() {
                        "minimum" => x >= b,
                        "maximum" => x <= b,
                        "exclusiveMinimum" => x > b,
                        _ => x < b,
                    };
                    if !ok {
                        errs.push(format!("{path}: {x} violates {key} {b}"));
                    }
                }
            }
            "minLength" | "maxLength" => {
                if let (Some(sv), Some(b)) = (v.as_str(), rule.as_u64()) {
                    let n = sv.chars().count() as u64;
                    if (key == "minLength" && n < b) || (key == "maxLength" && n > b) {
                        errs.push(format!("{path}: length {n} violates {key} {b}"));
                    }
                }
            }
            "minProperties" | "maxProperties" => {
                if let (Some(o), Some(b)) = (v.as_object(), rule.as_u64()) {
                    let n = o.len() as u64;
                    if (key == "minProperties" && n < b) || (key == "maxProperties" && n > b) {
                        errs.push(format!("{path}: {n} properties violates {key} {b}"));
                    }
                }
            }
            "minItems" | "maxItems" => {
                if let (Some(a), Some(b)) = (v.as_array(), rule.as_u64()) {
                    let n = a.len() as u64;
                    if (key == "minItems" && n < b) || (key == "maxItems" && n > b) {
                        errs.push(format!("{path}: {n} items violates {key} {b}"));
                    }
                }
            }
            "required" => {
                if let (Some(o), Some(req)) = (v.as_object(), rule.as_array()) {
                    for r in req.iter().filter_map(Value::as_str) {
                        if !o.contains_key(r) {
                            errs.push(format!("{path}: missing {r}"));
                        }
                    }
                }
            }
            "properties" => {
                if let (Some(o), Some(props)) = (v.as_object(), rule.as_object()) {
                    for (name, sub) in props {
                        if let Some(x) = o.get(name) {
                            check(docs, root, sub, x, &format!("{path}/{name}"), errs);
                        }
                    }
                }
            }
            "additionalProperties" => {
                if let Some(o) = v.as_object() {
                    let props = s.get("properties").and_then(Value::as_object);
                    for (name, x) in o {
                        if props.is_some_and(|p| p.contains_key(name)) {
                            continue;
                        }
                        match rule {
                            Value::Bool(false) => errs.push(format!("{path}: unexpected {name}")),
                            Value::Object(_) => {
                                check(docs, root, rule, x, &format!("{path}/{name}"), errs);
                            }
                            _ => {}
                        }
                    }
                }
            }
            "propertyNames" => {
                if let Some(o) = v.as_object() {
                    for name in o.keys() {
                        check(
                            docs,
                            root,
                            rule,
                            &Value::String(name.clone()),
                            &format!("{path}/{name}"),
                            errs,
                        );
                    }
                }
            }
            "items" => {
                if let Some(a) = v.as_array() {
                    for (i, x) in a.iter().enumerate() {
                        check(docs, root, rule, x, &format!("{path}/{i}"), errs);
                    }
                }
            }
            "anyOf" | "oneOf" => {
                let n = rule.as_array().map_or(0, |a| {
                    a.iter()
                        .filter(|sub| check_schema(docs, root, sub, v).is_ok())
                        .count()
                });
                if n == 0 || (key == "oneOf" && n != 1) {
                    errs.push(format!("{path}: {v} matches {n} of {key}"));
                }
            }
            "allOf" => {
                for sub in rule.as_array().into_iter().flatten() {
                    check(docs, root, sub, v, path, errs);
                }
            }
            other => errs.push(format!(
                "{path}: keyword {other} not supported by the test checker"
            )),
        }
    }
}

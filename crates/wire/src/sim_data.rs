//! Simulator data: the limits of a `sim_data` record and of a `sim_data_kinds` description, and
//! the checks that apply them locally, with the texts Pitwall's error replies use.
//!
//! A connector designs its own data structures (a *kind*) and describes every field of each
//! kind. Pitwall stores a record as sent and refuses one that breaks a limit; checking here
//! first means a connector learns of a bad record in its own log and tests instead of from a
//! refused write.

use crate::messages::{SimDataKind, SimDataKinds};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fmt;

/// Largest `data`, as compact UTF-8 JSON, in bytes.
pub const MAX_BYTES: usize = 16_384;
/// Deepest nesting of containers; `data` itself is 1.
pub const MAX_DEPTH: usize = 6;
/// Most keys in all the objects of `data` together.
pub const MAX_KEYS: usize = 512;
/// Most items of one array.
pub const MAX_ITEMS: usize = 256;
/// Longest string value, in characters.
pub const MAX_TEXT: usize = 500;
/// Most records Pitwall stores per session.
pub const MAX_PER_SESSION: u64 = 5_000;
/// Most kinds in one `sim_data_kinds` message.
pub const KINDS_MAX: usize = 32;
/// Most field paths of one kind.
pub const FIELDS_MAX: usize = 64;
/// Longest kind description, in characters.
pub const DESCRIPTION_MAX: usize = 500;
/// Longest field meaning, in characters.
pub const MEANING_MAX: usize = 300;
/// Longest field unit, in characters.
pub const UNIT_MAX: usize = 24;
/// Longest field path, in characters.
pub const PATH_MAX: usize = 128;
/// Most segments of a field path.
pub const PATH_SEGMENTS_MAX: usize = 6;
/// Longest key of `data` and longest segment of a field path.
pub const KEY_MAX: usize = 64;
/// The largest integer `data` may hold (2^53).
pub const SAFE_INTEGER: i64 = 1 << 53;
/// The topics a kind may name (the guidance topic vocabulary).
pub const TOPICS: [&str; 11] = [
    "setup",
    "capture",
    "telemetry",
    "session",
    "car",
    "track",
    "tyres",
    "fuel",
    "strategy",
    "controls",
    "other",
];

/// Why a record or a description is refused; the text is Pitwall's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimDataError(pub String);

impl fmt::Display for SimDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SimDataError {}

fn err<T>(text: impl Into<String>) -> Result<T, SimDataError> {
    Err(SimDataError(text.into()))
}

/// Whether `s` matches `^[a-z][a-z0-9_]{0,31}$`.
pub fn is_kind_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

fn is_key(s: &str) -> bool {
    (1..=KEY_MAX).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Whether `s` is a field path: one to six dotted segments of `[A-Za-z0-9_*-]{1,64}`, at most
/// 128 characters.
pub fn is_field_path(s: &str) -> bool {
    s.len() <= PATH_MAX
        && s.split('.').count() <= PATH_SEGMENTS_MAX
        && s.split('.').all(|seg| {
            (1..=KEY_MAX).contains(&seg.len())
                && seg
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'*' || c == b'-')
        })
}

fn at(path: &str) -> String {
    format!("sim_data.data.{path}")
}

fn walk(v: &Value, path: &str, depth: usize, keys: &mut usize) -> Result<(), SimDataError> {
    match v {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::String(s) => {
            if s.chars().count() > MAX_TEXT {
                return err(format!(
                    "{}: string longer than {MAX_TEXT} characters",
                    at(path)
                ));
            }
            Ok(())
        }
        Value::Number(n) => {
            let safe = if let Some(i) = n.as_i64() {
                (-SAFE_INTEGER..=SAFE_INTEGER).contains(&i)
            } else if n.is_u64() {
                false
            } else {
                n.as_f64().is_some_and(f64::is_finite)
            };
            if safe {
                Ok(())
            } else {
                err(format!("{}: not a finite number", at(path)))
            }
        }
        Value::Array(a) => {
            if depth > MAX_DEPTH {
                return err(format!("{}: nested deeper than {MAX_DEPTH}", at(path)));
            }
            if a.len() > MAX_ITEMS {
                return err(format!("{}: more than {MAX_ITEMS} items", at(path)));
            }
            for (i, x) in a.iter().enumerate() {
                walk(x, &format!("{path}.{i}"), depth + 1, keys)?;
            }
            Ok(())
        }
        Value::Object(o) => {
            if depth > MAX_DEPTH {
                return err(format!("{}: nested deeper than {MAX_DEPTH}", at(path)));
            }
            walk_object(o, path, depth, keys)
        }
    }
}

fn walk_object(
    o: &Map<String, Value>,
    path: &str,
    depth: usize,
    keys: &mut usize,
) -> Result<(), SimDataError> {
    for (k, x) in o {
        *keys += 1;
        if *keys > MAX_KEYS {
            return err(format!("sim_data.data: more than {MAX_KEYS} keys"));
        }
        let p = if path.is_empty() {
            k.clone()
        } else {
            format!("{path}.{k}")
        };
        if !is_key(k) {
            return err(format!("{}: bad key", at(&p)));
        }
        walk(x, &p, depth + 1, keys)?;
    }
    Ok(())
}

/// Checks a record's `kind` and `data` against the limits; `Err` carries Pitwall's text.
pub fn check_data(kind: &str, data: &Map<String, Value>) -> Result<(), SimDataError> {
    if !is_kind_name(kind) {
        return err("sim_data.kind: bad kind name");
    }
    walk_object(data, "", 1, &mut 0)?;
    let size = serde_json::to_string(data).map_or(usize::MAX, |t| t.len());
    if size > MAX_BYTES {
        return err(format!("sim_data.data: larger than {MAX_BYTES} bytes"));
    }
    Ok(())
}

fn text(what: &str, s: &str, max: usize) -> Result<(), SimDataError> {
    let n = s.chars().count();
    if n == 0 || n > max {
        return err(format!("sim_data_kinds.{what}: 1 to {max} characters"));
    }
    Ok(())
}

/// Checks a `sim_data_kinds` message: 1 to 32 distinct kinds, each with a description, an
/// optional topic of the vocabulary and 1 to 64 described field paths.
pub fn check_kinds(kinds: &[SimDataKind]) -> Result<(), SimDataError> {
    if kinds.is_empty() || kinds.len() > KINDS_MAX {
        return err(format!(
            "sim_data_kinds.kinds: 1 to {KINDS_MAX} kinds required"
        ));
    }
    let mut seen = HashSet::new();
    for k in kinds {
        let name = &k.kind;
        if !is_kind_name(name) {
            return err(format!("sim_data_kinds.kinds.{name}: bad kind name"));
        }
        if !seen.insert(name.as_str()) {
            return err(format!("sim_data_kinds.kinds.{name}: duplicate kind"));
        }
        text(
            &format!("kinds.{name}.description"),
            &k.description,
            DESCRIPTION_MAX,
        )?;
        if let Some(t) = &k.topic
            && !TOPICS.contains(&t.as_str())
        {
            return err(format!("sim_data_kinds.kinds.{name}.topic: unknown topic"));
        }
        if k.fields.is_empty() || k.fields.len() > FIELDS_MAX {
            return err(format!(
                "sim_data_kinds.kinds.{name}.fields: 1 to {FIELDS_MAX} fields required"
            ));
        }
        for (path, f) in &k.fields {
            if !is_field_path(path) {
                return err(format!(
                    "sim_data_kinds.kinds.{name}.fields.{path}: bad field path"
                ));
            }
            text(
                &format!("kinds.{name}.fields.{path}.meaning"),
                &f.meaning,
                MEANING_MAX,
            )?;
            if let Some(u) = &f.unit {
                text(&format!("kinds.{name}.fields.{path}.unit"), u, UNIT_MAX)?;
            }
        }
    }
    Ok(())
}

/// [`check_kinds`] of a whole message.
pub fn check_message(m: &SimDataKinds) -> Result<(), SimDataError> {
    check_kinds(&m.kinds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::SimDataField;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn refused(v: Value) -> String {
        check_data("stall_tyres", &map(v)).unwrap_err().0
    }

    /// An object nested so that `data` is at depth 1 and the innermost object at depth `n`.
    fn nest(n: usize) -> Value {
        let mut v = json!({"leaf": 1});
        for _ in 1..n {
            v = json!({ "a": v });
        }
        v
    }

    #[test]
    fn a_record_of_the_iracing_shape_passes() {
        let w = json!({"carcass_in_c": 74.1, "tread_mid": 0.9817});
        let d = map(json!({"occasion": "pit_stop",
            "wheels": {"fl": w, "fr": w, "rl": w, "rr": w}}));
        assert_eq!(check_data("stall_tyres", &d), Ok(()));
    }

    #[test]
    fn each_limit_is_refused_with_pitwalls_text() {
        assert_eq!(
            check_data("Bad", &Map::new()).unwrap_err().0,
            "sim_data.kind: bad kind name"
        );
        assert!(check_data("1a", &Map::new()).is_err());
        assert!(check_data(&"a".repeat(33), &Map::new()).is_err());
        assert!(check_data(&"a".repeat(32), &Map::new()).is_ok());
        // depth: data is 1, so objects down to depth 6 pass and a seventh does not
        assert!(check_data("k", &map(nest(6))).is_ok());
        assert_eq!(
            refused(nest(7)),
            "sim_data.data.a.a.a.a.a.a: nested deeper than 6"
        );
        assert_eq!(
            refused(json!({"a": [[[[[[1]]]]]]})),
            "sim_data.data.a.0.0.0.0.0: nested deeper than 6"
        );
        assert_eq!(
            refused(json!({"a": "x".repeat(501)})),
            "sim_data.data.a: string longer than 500 characters"
        );
        assert!(check_data("k", &map(json!({"a": "é".repeat(500)}))).is_ok());
        assert_eq!(
            refused(json!({"a": vec![0; 257]})),
            "sim_data.data.a: more than 256 items"
        );
        assert_eq!(refused(json!({"a.b": 1})), "sim_data.data.a.b: bad key");
        assert_eq!(refused(json!({"": 1})), "sim_data.data.: bad key");
        assert_eq!(
            refused(json!({"w": {"a b": 1}})),
            "sim_data.data.w.a b: bad key"
        );
        assert_eq!(
            refused(json!({"a": 9_007_199_254_740_993_i64})),
            "sim_data.data.a: not a finite number"
        );
        assert_eq!(
            refused(json!({"a": u64::MAX})),
            "sim_data.data.a: not a finite number"
        );
        let edge = map(json!({"a": 9_007_199_254_740_992_i64, "b": -1.5e300}));
        assert!(check_data("k", &edge).is_ok());
    }

    #[test]
    fn keys_are_counted_across_all_objects() {
        let group =
            |_: usize| Value::Object((0..63).map(|j| (format!("k{j}"), json!(j))).collect());
        let one: Map<String, Value> = (0..8).map(|i| (format!("g{i}"), group(i))).collect();
        // 8 + 8 * 63 = 512 keys
        assert!(check_data("k", &one).is_ok());
        let mut more = one;
        more.insert("extra".into(), json!(1));
        assert_eq!(
            check_data("k", &more).unwrap_err().0,
            "sim_data.data: more than 512 keys"
        );
    }

    #[test]
    fn size_is_compact_json_bytes() {
        let make = |count: usize| -> Map<String, Value> {
            (0..count)
                .map(|i| (format!("k{i:03}"), json!("x".repeat(100))))
                .collect()
        };
        // 100 chars + 2 quotes + 4 key + 2 quotes + colon + comma = 110 per entry (less one comma)
        assert!(check_data("k", &make(148)).is_ok());
        assert_eq!(
            check_data("k", &make(150)).unwrap_err().0,
            "sim_data.data: larger than 16384 bytes"
        );
    }

    fn kind(name: &str) -> SimDataKind {
        SimDataKind {
            kind: name.into(),
            description: "Tyre data given in the pits.".into(),
            topic: Some("tyres".into()),
            fields: BTreeMap::from([(
                "wheels.*.tread_mid".to_owned(),
                SimDataField {
                    meaning: "Tread remaining.".into(),
                    unit: Some("fraction".into()),
                },
            )]),
        }
    }

    fn field() -> SimDataField {
        kind("a").fields.remove("wheels.*.tread_mid").unwrap()
    }

    #[test]
    fn kinds_are_checked() {
        assert_eq!(check_kinds(&[kind("stall_tyres")]), Ok(()));
        assert!(check_kinds(&[]).is_err());
        let many: Vec<_> = (0..33).map(|i| kind(&format!("k{i}"))).collect();
        assert!(check_kinds(&many[..32]).is_ok());
        assert!(check_kinds(&many).is_err());
        assert!(
            check_kinds(&[kind("a"), kind("a")])
                .unwrap_err()
                .0
                .contains("duplicate")
        );
        assert!(check_kinds(&[kind("Bad")]).is_err());
        let mut k = kind("a");
        k.topic = Some("wheels".into());
        assert!(check_kinds(&[k]).unwrap_err().0.contains("topic"));
        let mut k = kind("a");
        k.topic = None;
        assert!(check_kinds(&[k]).is_ok());
        let mut k = kind("a");
        k.description = String::new();
        assert!(check_kinds(&[k]).is_err());
        let mut k = kind("a");
        k.description = "x".repeat(501);
        assert!(check_kinds(&[k]).is_err());
        let mut k = kind("a");
        k.fields.clear();
        assert!(check_kinds(&[k]).is_err());
        let mut k = kind("a");
        k.fields = (0..65).map(|i| (format!("f{i}"), field())).collect();
        assert!(check_kinds(&[k]).unwrap_err().0.contains("1 to 64"));
        let mut k = kind("a");
        k.fields = (0..64).map(|i| (format!("f{i}"), field())).collect();
        assert!(check_kinds(&[k]).is_ok());
        for bad in ["a..b", ".a", "a.", "a b", "a.b.c.d.e.f.g", ""] {
            let mut k = kind("a");
            k.fields = BTreeMap::from([(bad.to_owned(), field())]);
            let e = check_kinds(&[k]).unwrap_err().0;
            assert!(e.contains("bad field path"), "{bad}: {e}");
        }
        let mut k = kind("a");
        k.fields = BTreeMap::from([("a.b.c.d.e.f".to_owned(), field())]);
        assert!(check_kinds(&[k]).is_ok(), "six segments");
        let mut k = kind("a");
        let long = format!("{}.{}", "a".repeat(64), "b".repeat(64));
        k.fields = BTreeMap::from([(long, field())]);
        assert!(check_kinds(&[k]).unwrap_err().0.contains("bad field path"));
        let mut k = kind("a");
        k.fields.get_mut("wheels.*.tread_mid").unwrap().meaning = "x".repeat(301);
        assert!(check_kinds(&[k]).is_err());
        let mut k = kind("a");
        k.fields.get_mut("wheels.*.tread_mid").unwrap().unit = Some("x".repeat(25));
        assert!(check_kinds(&[k]).is_err());
    }
}

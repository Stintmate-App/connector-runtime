//! Messages: placeholders, the source hash, and the validation of a translation entry, of a
//! source message and of a pack file (docs/catalogs.md sections 4, 8 and 9).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::plural::{CATEGORIES, PluralRules};
use crate::reason::Reason;

/// `schema` of a catalog file.
pub const SCHEMA: &str = "stintmate-catalog/1";
/// `schema` of a language's `numbers.json`.
pub const NUMBERS_SCHEMA: &str = "stintmate-numbers/1";

const GENDERS: [&str; 3] = ["m", "f", "n"];
const SPOKEN_MAX: usize = 200;
/// The param kinds of docs/catalogs.md section 4.2.
pub const KINDS: [&str; 12] = [
    "text",
    "int",
    "decimal",
    "gap",
    "lap_time",
    "ordinal",
    "distance_m",
    "speed",
    "temperature",
    "litres",
    "laps",
    "enum",
];

/// A piece of a text: literal characters or a `{name}` placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Literal text (the `{{` and `}}` escapes already resolved).
    Lit(String),
    /// A placeholder name.
    Name(String),
}

fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// Splits a text into literals and placeholders. `Err` when a brace is neither an escape nor
/// a valid `{name}`.
pub fn parse(text: &str) -> Result<Vec<Segment>, Reason> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if rest.starts_with("{{") || rest.starts_with("}}") {
            lit.push_str(&rest[..1]);
            i += 2;
        } else if rest.starts_with('{') {
            let end = rest.find('}').ok_or(Reason::BadPlaceholder)?;
            let name = &rest[1..end];
            if !valid_name(name) {
                return Err(Reason::BadPlaceholder);
            }
            if !lit.is_empty() {
                out.push(Segment::Lit(std::mem::take(&mut lit)));
            }
            out.push(Segment::Name(name.to_string()));
            i += end + 1;
        } else if rest.starts_with('}') {
            return Err(Reason::BadPlaceholder);
        } else {
            let c = rest.chars().next().unwrap_or_default();
            lit.push(c);
            i += c.len_utf8();
        }
    }
    if !lit.is_empty() {
        out.push(Segment::Lit(lit));
    }
    Ok(out)
}

/// The placeholder names of a parsed text.
pub fn names(segments: &[Segment]) -> BTreeSet<String> {
    segments
        .iter()
        .filter_map(|s| match s {
            Segment::Name(n) => Some(n.clone()),
            Segment::Lit(_) => None,
        })
        .collect()
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::Bool(true)) => true,
    }
}

/// The first 12 hex digits of the SHA-256 of the source message's canonical JSON (sorted
/// keys, no spaces, UTF-8 as it is): `params` (empty object when absent), `text`, and
/// `values` when the message has them.
pub fn source_hash(msg: &Value) -> String {
    let mut obj = Map::new();
    let params = msg.get("params").filter(|p| truthy(Some(p)));
    obj.insert(
        "params".into(),
        params.cloned().unwrap_or_else(|| Value::Object(Map::new())),
    );
    obj.insert(
        "text".into(),
        msg.get("text").cloned().unwrap_or(Value::Null),
    );
    if let Some(values) = msg.get("values") {
        obj.insert("values".into(), values.clone());
    }
    // Sorted explicitly: another crate of the build may turn on `preserve_order`.
    let json = serde_json::to_string(&sorted(&Value::Object(obj))).unwrap_or_default();
    hex::encode(Sha256::digest(json.as_bytes()))[..12].to_string()
}

/// The kind of a param spec: the string itself, or the `kind` of an object spec.
pub fn kind_of(spec: Option<&Value>) -> Option<&str> {
    match spec? {
        Value::String(s) => Some(s),
        Value::Object(o) => o.get("kind")?.as_str(),
        _ => None,
    }
}

fn has_control(text: &str) -> bool {
    text.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}

fn is_hash(s: &str) -> bool {
    s.len() == 12
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `Ok` or the reason (docs/catalogs.md section 9) a translation `entry` of `key` is
/// rejected against its `source` message (`None`: not in the catalog) under the language's
/// plural `rules`.
pub fn validate(
    key: &str,
    source: Option<&Value>,
    entry: &Value,
    rules: &PluralRules,
) -> Result<(), Reason> {
    let source = source.ok_or(Reason::UnknownKey)?;
    let entry = entry.as_object().ok_or(Reason::BadText)?;
    let plural = source
        .get("plural")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty());
    let forms: BTreeMap<String, String> = match entry.get("text") {
        Some(Value::String(t)) if !t.is_empty() => BTreeMap::from([(
            if plural.is_some() { "other" } else { "" }.to_string(),
            t.clone(),
        )]),
        Some(Value::Object(o))
            if plural.is_some()
                && !o.is_empty()
                && o.values()
                    .all(|v| v.as_str().is_some_and(|s| !s.is_empty())) =>
        {
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        }
        _ => return Err(Reason::BadText),
    };
    if forms.values().any(|v| has_control(v)) {
        return Err(Reason::ControlCharacter);
    }
    let no_params = Map::new();
    let params = source
        .get("params")
        .and_then(Value::as_object)
        .unwrap_or(&no_params);
    let param_names: BTreeSet<String> = params.keys().cloned().collect();
    let mut used = BTreeMap::new();
    for (cat, text) in &forms {
        used.insert(cat.as_str(), names(&parse(text)?));
    }
    if let Some(count) = plural {
        let need: BTreeSet<String> = param_names
            .iter()
            .filter(|p| *p != count)
            .cloned()
            .collect();
        if used
            .values()
            .any(|u| !u.is_subset(&param_names) || !need.is_subset(u))
        {
            return Err(Reason::PlaceholderMismatch);
        }
        if !forms.contains_key("other") {
            return Err(Reason::PluralMissingOther);
        }
        if forms.keys().any(|c| c != "other" && !rules.contains_key(c)) {
            return Err(Reason::PluralBadCategory);
        }
    } else if used.get("") != Some(&param_names) {
        return Err(Reason::PlaceholderMismatch);
    }
    if let Some(values) = entry.get("values") {
        let no_values = Map::new();
        let source_values = source
            .get("values")
            .and_then(Value::as_object)
            .unwrap_or(&no_values);
        let values = values.as_object().ok_or(Reason::EnumValuesMismatch)?;
        for (param, mapping) in values {
            let want = source_values.get(param).ok_or(Reason::EnumValuesMismatch)?;
            let (Some(have), Some(want)) = (mapping.as_object(), want.as_object()) else {
                return Err(Reason::EnumValuesMismatch);
            };
            if !have.keys().eq(want.keys()) {
                return Err(Reason::EnumValuesMismatch);
            }
        }
    }
    if let Some(forms_) = entry.get("forms") {
        let bad = forms_.as_object().is_none_or(|fm| {
            fm.iter().any(|(p, g)| {
                !matches!(kind_of(params.get(p)), Some("int" | "ordinal"))
                    || !g.as_str().is_some_and(|g| GENDERS.contains(&g))
            })
        });
        if bad {
            return Err(Reason::BadForms);
        }
    }
    if key.starts_with("speech.") && forms.values().any(|v| v.chars().count() > SPOKEN_MAX) {
        return Err(Reason::TooLong);
    }
    if let Some(hash) = entry.get("source_hash")
        && !hash.as_str().is_some_and(is_hash)
    {
        return Err(Reason::BadSourceHash);
    }
    Ok(())
}

fn valid_key(key: &str) -> bool {
    let seg = |s: &str, first_letter: bool| {
        (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            && (!first_letter || s.as_bytes()[0].is_ascii_lowercase())
    };
    let mut parts = key.split('.');
    let head = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    seg(head, true) && rest.len() <= 5 && rest.iter().all(|p| seg(p, false))
}

fn spec_ok(spec: &Value) -> bool {
    let Some(kind) = kind_of(Some(spec)).filter(|k| KINDS.contains(k)) else {
        return false;
    };
    let Value::Object(o) = spec else {
        return kind != "enum";
    };
    match kind {
        "decimal" => o
            .get("digits")
            .is_none_or(|d| d.as_i64().is_some_and(|d| (0..=6).contains(&d))),
        "enum" => match o.get("values").and_then(Value::as_array) {
            Some(v) => {
                let strs: Vec<&str> = v.iter().filter_map(Value::as_str).collect();
                !v.is_empty()
                    && strs.len() == v.len()
                    && strs.iter().all(|s| !s.is_empty())
                    && strs.iter().collect::<BTreeSet<_>>().len() == strs.len()
            }
            None => false,
        },
        _ => true,
    }
}

/// `Ok` or why a source message (a built-in one) cannot be used. Reasons: `bad_key`,
/// `bad_text`, `control_character`, `bad_placeholder`, `bad_params`, `bad_plural`,
/// `placeholder_mismatch`, `plural_missing_other`, `plural_bad_category`,
/// `enum_values_mismatch`, `bad_forms`, `too_long`.
pub fn validate_source(key: &str, msg: &Value) -> Result<(), Reason> {
    if !valid_key(key) {
        return Err(Reason::BadKey);
    }
    let obj = msg.as_object().ok_or(Reason::BadText)?;
    let no_params = Map::new();
    let params = match obj.get("params") {
        None | Some(Value::Null) => &no_params,
        Some(Value::Object(p)) => p,
        Some(_) => return Err(Reason::BadParams),
    };
    if !params
        .iter()
        .all(|(name, spec)| valid_name(name) && spec_ok(spec))
    {
        return Err(Reason::BadParams);
    }
    let plural = obj.get("plural").filter(|p| !p.is_null());
    let text = obj.get("text");
    if let Some(p) = plural {
        let count = p.as_str().and_then(|p| params.get(p));
        if !matches!(kind_of(count), Some("int" | "laps")) {
            return Err(Reason::BadPlural);
        }
    }
    if let Some(Value::Object(forms)) = text {
        if plural.is_none() {
            return Err(Reason::BadPlural);
        }
        if forms
            .keys()
            .any(|c| c != "other" && !CATEGORIES.contains(&c.as_str()))
        {
            return Err(Reason::PluralBadCategory);
        }
    }
    let enums: BTreeMap<&str, Vec<&str>> = params
        .iter()
        .filter(|(_, s)| kind_of(Some(s)) == Some("enum"))
        .map(|(p, s)| {
            let list = s
                .get("values")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            (p.as_str(), list.filter_map(Value::as_str).collect())
        })
        .collect();
    let no_values = Map::new();
    let values = match obj.get("values") {
        None | Some(Value::Null) => &no_values,
        Some(Value::Object(v)) => v,
        Some(_) => return Err(Reason::EnumValuesMismatch),
    };
    let same_set = values.keys().map(String::as_str).eq(enums.keys().copied());
    let each_ok = enums.iter().all(|(p, list)| {
        values.get(*p).and_then(Value::as_object).is_some_and(|m| {
            let have: BTreeSet<&str> = m.keys().map(String::as_str).collect();
            have == list.iter().copied().collect::<BTreeSet<_>>()
                && m.values()
                    .all(|t| t.as_str().is_some_and(|t| !t.is_empty()))
        })
    });
    if !same_set || !each_ok {
        return Err(Reason::EnumValuesMismatch);
    }
    // The text checks are those of a translation of itself (all categories allowed).
    let mut entry = Map::new();
    for k in ["text", "forms"] {
        if let Some(v) = obj.get(k) {
            entry.insert(k.to_string(), v.clone());
        }
    }
    let all: PluralRules = CATEGORIES
        .iter()
        .map(|c| (c.to_string(), String::new()))
        .collect();
    validate(key, Some(msg), &Value::Object(entry), &all)
}

/// `Ok` or why a pack file is skipped as a whole: `doc` is its parsed JSON, `folder` its
/// folder name, `catalog` the catalog it must belong to.
pub fn check_file(doc: &Value, folder: &str, catalog: &str) -> Result<(), Reason> {
    let doc = doc.as_object().ok_or(Reason::NotJson)?;
    if doc.get("schema").and_then(Value::as_str) != Some(SCHEMA)
        || !doc.get("messages").is_some_and(Value::is_object)
    {
        return Err(Reason::BadSchema);
    }
    if doc.get("catalog").and_then(Value::as_str) != Some(catalog) {
        return Err(Reason::CatalogMismatch);
    }
    let language = doc
        .get("language")
        .and_then(Value::as_str)
        .and_then(crate::normalize_tag);
    match language {
        Some(l) if l.to_lowercase() == folder.to_lowercase() => Ok(()),
        _ => Err(Reason::LanguageMismatch),
    }
}

/// A copy of `v` with every object's keys in sorted order, whatever the `serde_json` map is
/// (another crate of the build may turn on `preserve_order`).
pub(crate) fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), sorted(&m[k])))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

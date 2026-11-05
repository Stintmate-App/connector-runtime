//! The announcement text catalog format of a connector.
//!
//! A connector never composes user text: it declares its catalog with `pitwall.catalog.declare`
//! at every connection and then sends only a code and its params (`pitwall.announce`). The
//! catalog is data, in the catalog format of the connector kit: `text` with `{name}`
//! placeholders, `params`, `values` for `enum` params, a `note`, and per message `audience`,
//! `priority`, `max_wait_s`, `slot`, `kind` and `group`. The source hash of a message and the
//! idempotency key of the whole catalog are computed over canonical JSON: sorted keys, no
//! spaces, no ASCII escaping.
//!
//! [`AnnounceCatalog`] holds a validated catalog; the free functions are the format's pieces.

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Catalog format.
pub const SCHEMA: &str = "stintmate-catalog/1";
/// Valid audiences.
pub const AUDIENCES: [&str; 2] = ["driver", "agent"];
/// Valid groups (contract 4.4).
pub const GROUPS: [&str; 12] = [
    "laps",
    "fuel",
    "setup",
    "flags",
    "safety_car",
    "penalties",
    "race",
    "rivals",
    "car",
    "test",
    "spotter",
    "other",
];
/// Valid priorities (contract: `pitwall.catalog.declare`).
pub const PRIORITIES: [&str; 5] = ["low", "normal", "high", "critical", "spotter"];
/// Priorities an `agent` message may use.
pub const AGENT_PRIORITIES: [&str; 3] = ["low", "normal", "high"];
/// Valid message kinds.
pub const KINDS: [&str; 2] = ["announcement", "message"];
/// Smallest `max_wait_s`.
pub const MAX_WAIT_MIN: f64 = 0.5;
/// Largest `max_wait_s`.
pub const MAX_WAIT_MAX: f64 = 600.0;
/// Largest `max_wait_s` of a `spotter` message.
pub const SPOTTER_MAX_WAIT: f64 = 3.0;
/// Longest `slot`.
pub const SLOT_MAX: usize = 32;
/// Most `variants` a message may declare.
pub const MAX_VARIANTS: usize = 8;
/// Longest filled text.
pub const MAX_TEXT: usize = 200;
/// Longest value a text param is expected to carry when filled (a driver's name).
pub const TYPICAL_TEXT_PARAM: usize = 32;
/// Param kinds of the format.
pub const PARAM_KINDS: [&str; 12] = [
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
/// `validation` status of a code only seen in synthetic tests.
pub const NEEDS_RACE: &str = "needs a real race";
/// `validation` status of a code tried by hand.
pub const DESK_TESTED: &str = "desk-tested";

/// A connector's announcement catalog, validated once when it is parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct AnnounceCatalog {
    doc: Value,
}

impl AnnounceCatalog {
    /// Parses and validates a catalog document whose `catalog` field must be `expected_id`.
    /// The error lists every problem, as English lines.
    pub fn parse(json: &str, expected_id: &str) -> Result<Self, Vec<String>> {
        let doc: Value = serde_json::from_str(json).map_err(|e| vec![format!("not JSON: {e}")])?;
        let problems = validate_catalog(&doc, expected_id);
        if problems.is_empty() {
            Ok(Self { doc })
        } else {
            Err(problems)
        }
    }

    /// The whole catalog document.
    pub fn doc(&self) -> &Value {
        &self.doc
    }

    /// The messages by code.
    pub fn messages(&self) -> &Map<String, Value> {
        self.doc["messages"]
            .as_object()
            .expect("a parsed catalog has messages")
    }

    /// Whether `code` is a `spotter`-level message (its own, larger rate window).
    pub fn is_spotter(&self, code: &str) -> bool {
        self.messages()
            .get(code)
            .and_then(|m| m.get("priority"))
            .and_then(Value::as_str)
            == Some("spotter")
    }

    /// How many wordings `code` has: its `text` plus its `variants` (1 when it declares none, and
    /// for an unknown code). No allocation.
    pub fn variant_count(&self, code: &str) -> u8 {
        let n = self
            .messages()
            .get(code)
            .and_then(|m| m.get("variants"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        u8::try_from(1 + n).unwrap_or(u8::MAX)
    }

    /// The catalog's codes, sorted.
    pub fn codes(&self) -> Vec<&str> {
        let mut codes: Vec<&str> = self.messages().keys().map(String::as_str).collect();
        codes.sort_unstable(); // sorted whatever the JSON map keeps
        codes
    }

    /// `NAME=VALUE` pairs typed by the code's param kinds (`int`, `ordinal`, `laps` as integers,
    /// the number kinds as floats, the rest as text). The error is the English line to show.
    pub fn parse_params(&self, code: &str, pairs: &[String]) -> Result<Map<String, Value>, String> {
        let declared = self
            .messages()
            .get(code)
            .and_then(|m| m.get("params"))
            .and_then(Value::as_object);
        let mut out = Map::new();
        for pair in pairs {
            let Some((name, value)) = pair.split_once('=').filter(|(n, _)| !n.is_empty()) else {
                return Err(format!("--param {pair:?}: use NAME=VALUE"));
            };
            let kind = declared.and_then(|d| d.get(name)).and_then(param_kind);
            let typed = match kind {
                Some("int" | "ordinal" | "laps") => value
                    .trim()
                    .parse::<i64>()
                    .map(Value::from)
                    .map_err(|_| format!("invalid literal for int() with base 10: {value:?}"))?,
                Some(
                    "decimal" | "gap" | "lap_time" | "distance_m" | "speed" | "temperature"
                    | "litres",
                ) => value
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
                    .ok_or_else(|| format!("could not convert string to float: {value:?}"))?,
                _ => Value::String(value.to_owned()),
            };
            out.insert(name.to_owned(), typed);
        }
        Ok(out)
    }

    /// What has been checked for a code: [`DESK_TESTED`] when `desk_tested` lists it,
    /// [`NEEDS_RACE`] for any other code of the catalog, `None` for what is not a code.
    pub fn validation(&self, code: &str, desk_tested: &[&str]) -> Option<&'static str> {
        if !self.messages().contains_key(code) {
            None
        } else if desk_tested.contains(&code) {
            Some(DESK_TESTED)
        } else {
            Some(NEEDS_RACE)
        }
    }

    /// The idempotency key of the catalog declaration: `<prefix>/textcatalog/<sha12 of the
    /// catalog>`.
    pub fn ik(&self, prefix: &str) -> String {
        format!("{prefix}/textcatalog/{}", sha12(&canonical(&self.doc)))
    }
}

/// Canonical JSON: sorted keys, no spaces, non-ASCII kept (Python's
/// `json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=False)`).
pub fn canonical(v: &Value) -> String {
    serde_json::to_string(&sorted(v)).expect("a JSON value serializes")
}

/// The first 12 hex digits of the SHA-256 of `text`.
pub fn sha12(text: &str) -> String {
    let mut h = hex::encode(Sha256::digest(text.as_bytes()));
    h.truncate(12);
    h
}

fn is_falsy(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(m) => m.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// The source hash of a message (catalogs.md section 8): the first 12 hex digits of the SHA-256
/// of `{"params", "text"[, "values"]}` as canonical JSON.
pub fn source_hash(message: &Value) -> String {
    let mut obj = Map::new();
    let params = match message.get("params") {
        Some(p) if !is_falsy(p) => p.clone(),
        _ => Value::Object(Map::new()),
    };
    obj.insert("params".into(), params);
    obj.insert("text".into(), message["text"].clone());
    if let Some(v) = message.get("values") {
        obj.insert("values".into(), v.clone());
    }
    sha12(&canonical(&Value::Object(obj)))
}

fn param_kind(spec: &Value) -> Option<&str> {
    match spec {
        Value::String(s) => Some(s),
        Value::Object(m) => m.get("kind").and_then(Value::as_str),
        _ => None,
    }
}

fn is_int(v: &Value) -> bool {
    v.as_number().is_some_and(|n| n.is_i64() || n.is_u64())
}

/// `^[a-z][a-z0-9_]{0,31}$`: codes and param names.
fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=32).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

/// The `{name}` placeholders of a text, or `None` when it does not parse (catalogs.md 4.1:
/// `{{` and `}}` are literal braces, any other brace is an error).
pub fn placeholders(text: &str) -> Option<Vec<String>> {
    let b = text.as_bytes();
    let mut names = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' => {
                if b.get(i + 1) == Some(&b'{') {
                    i += 2;
                    continue;
                }
                let end = i + 1 + text[i + 1..].find('}')?;
                let name = &text[i + 1..end];
                if !valid_name(name) {
                    return None;
                }
                names.push(name.to_string());
                i = end + 1;
            }
            b'}' => {
                if b.get(i + 1) == Some(&b'}') {
                    i += 2;
                    continue;
                }
                return None;
            }
            _ => i += 1,
        }
    }
    Some(names)
}

fn has_control(text: &str) -> bool {
    text.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7F)
}

/// `None` when `params` fit `code`'s declaration, else an English reason naming the param (the
/// connector checks before sending; Pitwall refuses the same with `bad_request`).
pub fn check_params(code: &str, params: &Value, messages: &Map<String, Value>) -> Option<String> {
    let Some(msg) = messages.get(code) else {
        return Some(format!("unknown code {code:?}"));
    };
    let Some(given) = params.as_object() else {
        return Some("params must be a map".into());
    };
    let empty = Map::new();
    let declared = msg
        .get("params")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for name in given.keys() {
        if !declared.contains_key(name) {
            return Some(format!("param {name:?} is not declared for {code}"));
        }
    }
    for (name, spec) in declared {
        let Some(value) = given.get(name) else {
            return Some(format!("param {name:?} is missing for {code}"));
        };
        match param_kind(spec) {
            Some("text") => {
                if !value.is_string() {
                    return Some(format!("param {name:?} must be text"));
                }
            }
            Some("int" | "laps") => {
                if !is_int(value) {
                    return Some(format!("param {name:?} must be an integer"));
                }
            }
            Some("ordinal") => {
                if !is_int(value) || value.as_i64().is_none_or(|n| n < 1) {
                    return Some(format!("param {name:?} must be an integer >= 1"));
                }
            }
            Some("enum") => {
                let allowed = spec
                    .get("values")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if !allowed.contains(value) {
                    // The value is never echoed: it may hold what the user said, and this
                    // text reaches the log. The allowed values come from the catalog.
                    return Some(format!(
                        "param {name:?} is not one of {}",
                        Value::Array(allowed)
                    ));
                }
            }
            _ => {
                if !value
                    .as_number()
                    .is_some_and(|n| n.as_f64().is_some_and(f64::is_finite))
                {
                    return Some(format!("param {name:?} must be a number"));
                }
            }
        }
    }
    None
}

/// Every problem of a connector catalog, as English lines (empty: valid): the checks of
/// catalogs.md sections 2-4 and 9 that apply to a source catalog, plus the connector's own fields.
pub fn validate_catalog(catalog: &Value, expected_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    if catalog.get("schema").and_then(Value::as_str) != Some(SCHEMA) {
        out.push(format!("schema must be {SCHEMA}"));
    }
    if catalog.get("catalog").and_then(Value::as_str) != Some(expected_id) {
        out.push(format!("catalog must be {expected_id}"));
    }
    if !catalog
        .get("language")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        out.push("language is required".into());
    }
    let Some(messages) = catalog
        .get("messages")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
    else {
        out.push("messages must be a non-empty map".into());
        return out;
    };
    let empty = Map::new();
    for (code, msg) in messages {
        let at = format!("{code}:");
        if !valid_name(code) {
            out.push(format!("{at} code does not match ^[a-z][a-z0-9_]{{0,31}}$"));
        }
        let Some(msg) = msg.as_object() else {
            out.push(format!("{at} not a map"));
            continue;
        };
        let Some(text) = msg
            .get("text")
            .and_then(Value::as_str)
            .filter(|t| !t.trim().is_empty())
        else {
            out.push(format!("{at} text must be a non-empty string"));
            continue;
        };
        if has_control(text) {
            out.push(format!("{at} control character in text"));
        }
        let params = msg
            .get("params")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        match placeholders(text) {
            None => out.push(format!("{at} bad placeholder")),
            Some(names) => {
                let mut used: Vec<&str> = names.iter().map(String::as_str).collect();
                used.sort_unstable();
                used.dedup();
                let declared: Vec<&str> = params.keys().map(String::as_str).collect();
                if used != declared {
                    out.push(format!(
                        "{at} placeholders {used:?} differ from params {declared:?}"
                    ));
                }
            }
        }
        let mut filled = text.chars().count();
        for (name, spec) in params {
            let kind = param_kind(spec);
            if !kind.is_some_and(|k| PARAM_KINDS.contains(&k)) {
                out.push(format!(
                    "{at} param {name:?} has an unknown kind {:?}",
                    kind.unwrap_or("None")
                ));
                continue;
            }
            match kind {
                Some("text") => filled += TYPICAL_TEXT_PARAM,
                Some("enum") => {
                    let allowed: Vec<&str> = spec
                        .get("values")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let texts = msg
                        .get("values")
                        .and_then(|v| v.get(name))
                        .and_then(Value::as_object);
                    match texts {
                        Some(t)
                            if !allowed.is_empty()
                                && t.len() == allowed.len()
                                && allowed.iter().all(|a| t.contains_key(*a)) =>
                        {
                            let strs: Vec<&str> = t.values().filter_map(Value::as_str).collect();
                            filled += strs.iter().map(|s| s.chars().count()).max().unwrap_or(0);
                            if strs.iter().any(|s| has_control(s)) {
                                out.push(format!("{at} control character in values of {name:?}"));
                            }
                        }
                        _ => out.push(format!(
                            "{at} enum {name:?} needs values with a text for each"
                        )),
                    }
                }
                _ => filled += 8,
            }
        }
        if filled > MAX_TEXT {
            out.push(format!(
                "{at} longer than {MAX_TEXT} characters when filled"
            ));
        }
        if let Some(v) = msg.get("variants") {
            let extra = filled - text.chars().count();
            match v.as_array() {
                Some(list) if (1..=MAX_VARIANTS).contains(&list.len()) => {
                    let mut seen: Vec<&str> = vec![text];
                    for (i, item) in list.iter().enumerate() {
                        let n = i + 1;
                        let Some(vt) = item.as_str().filter(|t| !t.trim().is_empty()) else {
                            out.push(format!("{at} variant {n} must be a non-empty string"));
                            continue;
                        };
                        if has_control(vt) {
                            out.push(format!("{at} control character in variant {n}"));
                        }
                        match placeholders(vt) {
                            None => out.push(format!("{at} bad placeholder in variant {n}")),
                            Some(names) => {
                                let mut used: Vec<&str> =
                                    names.iter().map(String::as_str).collect();
                                used.sort_unstable();
                                used.dedup();
                                let declared: Vec<&str> =
                                    params.keys().map(String::as_str).collect();
                                if used != declared {
                                    out.push(format!(
                                        "{at} variant {n} placeholders {used:?} differ from params {declared:?}"
                                    ));
                                }
                            }
                        }
                        if vt.chars().count() + extra > MAX_TEXT {
                            out.push(format!(
                                "{at} variant {n} longer than {MAX_TEXT} characters when filled"
                            ));
                        }
                        if seen.contains(&vt) {
                            out.push(format!("{at} variant {n} repeats another wording"));
                        }
                        seen.push(vt);
                    }
                }
                _ => out.push(format!(
                    "{at} variants must be a list of 1..{MAX_VARIANTS} texts"
                )),
            }
        }
        let in_list = |key: &str, list: &[&str]| {
            msg.get(key)
                .and_then(Value::as_str)
                .is_some_and(|a| list.contains(&a))
        };
        if !in_list("audience", &AUDIENCES) {
            out.push(format!("{at} audience must be one of {AUDIENCES:?}"));
        }
        if !in_list("priority", &PRIORITIES) {
            out.push(format!("{at} priority must be one of {PRIORITIES:?}"));
        }
        if let Some(p) = msg.get("priority").and_then(Value::as_str) {
            if p == "spotter" && msg.get("audience").and_then(Value::as_str) != Some("driver") {
                out.push(format!("{at} priority spotter needs audience driver"));
            }
            if p == "spotter" && !msg.contains_key("max_wait_s") {
                out.push(format!("{at} priority spotter needs max_wait_s"));
            }
            if msg.get("audience").and_then(Value::as_str) == Some("agent")
                && !AGENT_PRIORITIES.contains(&p)
            {
                out.push(format!(
                    "{at} priority of an agent message must be one of {AGENT_PRIORITIES:?}"
                ));
            }
        }
        if msg.contains_key("ttl_s") {
            out.push(format!("{at} ttl_s is gone: use max_wait_s"));
        }
        if let Some(w) = msg.get("max_wait_s") {
            let max = if msg.get("priority").and_then(Value::as_str) == Some("spotter") {
                SPOTTER_MAX_WAIT
            } else {
                MAX_WAIT_MAX
            };
            if !w
                .as_f64()
                .is_some_and(|w| (MAX_WAIT_MIN..=max).contains(&w))
            {
                out.push(format!(
                    "{at} max_wait_s must be a number {MAX_WAIT_MIN}..{max}"
                ));
            }
        }
        if let Some(sl) = msg.get("slot")
            && !sl.as_str().is_some_and(valid_slot)
        {
            out.push(format!(
                "{at} slot must match ^[a-z][a-z0-9_]{{0,{}}}$",
                SLOT_MAX - 1
            ));
        }
        if !in_list("group", &GROUPS) {
            out.push(format!("{at} group must be one of {GROUPS:?}"));
        }
        if msg.contains_key("kind") && !in_list("kind", &KINDS) {
            out.push(format!("{at} kind must be one of {KINDS:?}"));
        }
        if let Some(n) = msg.get("note")
            && !n.as_str().is_some_and(|n| !has_control(n))
        {
            out.push(format!("{at} note must be plain text"));
        }
    }
    out
}

/// `^[a-z][a-z0-9_]{0,31}$`.
fn valid_slot(s: &str) -> bool {
    let mut it = s.bytes();
    it.next().is_some_and(|b| b.is_ascii_lowercase())
        && s.len() <= SLOT_MAX
        && it.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// A copy of `v` with every object's keys in sorted order, whatever the `serde_json` map is
/// (another crate of the build may turn on `preserve_order`).
fn sorted(v: &Value) -> Value {
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

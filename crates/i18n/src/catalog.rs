//! One resolved catalog: the source texts with the packs of the language's chain over them.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{Map, Value};

use crate::message::{Segment, kind_of, parse, source_hash, validate, validate_source};
use crate::plural::{CompiledRules, PluralRules, compile_rules, english_rules};
use crate::reason::Reason;
use crate::tag::{chain, normalize_tag};

/// A value for a placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    /// An integer (written as plain digits, no grouping; selects the plural form).
    Int(i64),
    /// A string (text, or the value of an enum param).
    Text(String),
}

impl Arg {
    fn written(&self) -> String {
        match self {
            Arg::Int(n) => n.to_string(),
            Arg::Text(s) => s.clone(),
        }
    }
}

impl From<i64> for Arg {
    fn from(n: i64) -> Self {
        Arg::Int(n)
    }
}

impl From<i32> for Arg {
    fn from(n: i32) -> Self {
        Arg::Int(n.into())
    }
}

impl From<u32> for Arg {
    fn from(n: u32) -> Self {
        Arg::Int(n.into())
    }
}

impl From<&str> for Arg {
    fn from(s: &str) -> Self {
        Arg::Text(s.to_string())
    }
}

impl From<String> for Arg {
    fn from(s: String) -> Self {
        Arg::Text(s)
    }
}

/// A message resolved once: parsed forms and what filling needs.
#[derive(Debug, Clone)]
struct Resolved {
    /// Plural category (`""` for a message without plural) to its parsed text.
    forms: BTreeMap<String, Vec<Segment>>,
    plural: Option<String>,
    kinds: BTreeMap<String, Option<String>>,
    /// Enum param to value to text.
    values: BTreeMap<String, BTreeMap<String, String>>,
}

/// The texts of one language: the source catalog with the packs of the chain over it, per key.
#[derive(Debug, Clone)]
pub struct Catalog {
    language: String,
    rules: PluralRules,
    compiled: CompiledRules,
    source: Map<String, Value>,
    messages: HashMap<String, Resolved>,
    /// Pack tag to key to the reason the entry was dropped.
    pub rejected: BTreeMap<String, BTreeMap<String, Reason>>,
    /// Source messages that cannot be used, with the reason (they are left out).
    pub invalid: BTreeMap<String, Reason>,
    /// Keys whose translation has no `source_hash` or a different one.
    pub stale: BTreeSet<String>,
    /// Keys answered by a pack entry.
    pub translated: BTreeSet<String>,
}

impl Catalog {
    /// The English catalog as it is (no packs, English plural rules).
    pub fn english(builtin: &Map<String, Value>) -> Catalog {
        Catalog::new(builtin, "en", &BTreeMap::new(), None)
    }

    /// Resolves `builtin` (source key to message) for `language`: `packs` maps a tag of the
    /// language's [`chain`] to that pack's messages, the most specific tag winning per key
    /// (`es-CL` over `es`, then the built-in). Entries are validated against the source and
    /// `rules` (English when `None`); the failures are in [`Catalog::rejected`].
    pub fn new(
        builtin: &Map<String, Value>,
        language: &str,
        packs: &BTreeMap<String, Map<String, Value>>,
        rules: Option<PluralRules>,
    ) -> Catalog {
        let language = normalize_tag(language).unwrap_or_else(|| "en".to_string());
        let rules = rules.unwrap_or_else(english_rules);
        let mut cat = Catalog {
            language,
            compiled: compile_rules(&rules),
            rules,
            source: Map::new(),
            messages: HashMap::new(),
            rejected: BTreeMap::new(),
            invalid: BTreeMap::new(),
            stale: BTreeSet::new(),
            translated: BTreeSet::new(),
        };
        for (key, msg) in builtin {
            match validate_source(key, msg) {
                Ok(()) => {
                    cat.source.insert(key.clone(), msg.clone());
                }
                Err(why) => {
                    cat.invalid.insert(key.clone(), why);
                }
            }
        }
        let mut chosen: BTreeMap<&str, &Value> = BTreeMap::new();
        for tag in chain(&cat.language).iter().rev() {
            let Some(messages) = packs.get(tag) else {
                continue;
            };
            for (key, entry) in messages {
                match validate(key, cat.source.get(key), entry, &cat.rules) {
                    Ok(()) => {
                        chosen.insert(key, entry);
                    }
                    Err(why) => {
                        cat.rejected
                            .entry(tag.clone())
                            .or_default()
                            .insert(key.clone(), why);
                    }
                }
            }
        }
        for (key, msg) in &cat.source {
            let entry = chosen.get(key.as_str()).copied().unwrap_or(msg);
            if chosen.contains_key(key.as_str()) {
                cat.translated.insert(key.clone());
                if entry.get("source_hash").and_then(Value::as_str) != Some(&source_hash(msg)) {
                    cat.stale.insert(key.clone());
                }
            }
            if let Some(resolved) = resolve(msg, entry) {
                cat.messages.insert(key.clone(), resolved);
            }
        }
        cat
    }

    /// The normalised language tag of the catalog.
    pub fn language(&self) -> &str {
        &self.language
    }

    /// The plural rules in force.
    pub fn rules(&self) -> &PluralRules {
        &self.rules
    }

    /// The source messages in use.
    pub fn source(&self) -> &Map<String, Value> {
        &self.source
    }

    /// True when `key` is a usable source message.
    pub fn contains(&self, key: &str) -> bool {
        self.messages.contains_key(key)
    }

    /// The text of `key` with its placeholders filled. An unknown key answers the key itself;
    /// a placeholder without a value stays as `{name}`.
    pub fn text(&self, key: &str, params: &[(&str, Arg)]) -> String {
        let Some(m) = self.messages.get(key) else {
            return key.to_string();
        };
        let segments = match &m.plural {
            None => m.forms.get(""),
            Some(count) => {
                let cat = match params.iter().find(|(n, _)| n == count) {
                    Some((_, Arg::Int(n))) => self.compiled.category(*n),
                    _ => "other",
                };
                m.forms.get(cat).or_else(|| m.forms.get("other"))
            }
        };
        let mut out = String::new();
        for seg in segments.into_iter().flatten() {
            match seg {
                Segment::Lit(s) => out.push_str(s),
                Segment::Name(name) => match params.iter().find(|(n, _)| n == name) {
                    Some((_, arg)) => out.push_str(&render(m, name, arg)),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                },
            }
        }
        out
    }
}

fn render(m: &Resolved, name: &str, arg: &Arg) -> String {
    let written = arg.written();
    if m.kinds.get(name).and_then(|k| k.as_deref()) == Some("enum")
        && let Some(text) = m.values.get(name).and_then(|v| v.get(&written))
    {
        return text.clone();
    }
    written
}

fn resolve(source: &Value, entry: &Value) -> Option<Resolved> {
    let text = entry.get("text")?;
    let plural = source
        .get("plural")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut forms = BTreeMap::new();
    match (&plural, text) {
        (None, Value::String(t)) => {
            forms.insert(String::new(), parse(t).ok()?);
        }
        (Some(_), Value::String(t)) => {
            forms.insert("other".to_string(), parse(t).ok()?);
        }
        (Some(_), Value::Object(o)) => {
            for (cat, v) in o {
                forms.insert(cat.clone(), parse(v.as_str()?).ok()?);
            }
        }
        _ => return None,
    }
    let no_params = Map::new();
    let params = source
        .get("params")
        .and_then(Value::as_object)
        .unwrap_or(&no_params);
    let kinds = params
        .iter()
        .map(|(n, s)| (n.clone(), kind_of(Some(s)).map(str::to_string)))
        .collect();
    let mut values: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for src in [source.get("values"), entry.get("values")] {
        for (param, mapping) in src.and_then(Value::as_object).into_iter().flatten() {
            let texts = mapping
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect();
            values.insert(param.clone(), texts);
        }
    }
    Some(Resolved {
        forms,
        plural,
        kinds,
        values,
    })
}

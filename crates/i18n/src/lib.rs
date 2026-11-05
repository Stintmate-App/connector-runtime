//! The connector texts runtime: the connector kit's text catalogs format (catalogs.md).
//!
//! The chain of catalogs is the connector's embedded English catalog ([`Embedded`]) with the
//! workspace's language packs over it ([`load`]), per key, every pack entry validated with
//! the contract's reason codes ([`Reason`]). Plural rules are the CLDR subset integer counts
//! need ([`plural`]); source hashes mark stale translations ([`source_hash`]). The test
//! pseudo-locale (`qps`) is [`pseudo_pack`].
//!
//! The English texts and the catalog id belong to the connector: it embeds its own catalog
//! file and hands it to [`Embedded::parse`]. Not localised: logs, JSON files, IPC error texts
//! and ids, the connector descriptor. The recorder never loads a catalog on a hot path; the
//! control app loads it once at start.

#![deny(missing_docs)]

mod catalog;
mod load;
mod message;
pub mod plural;
mod reason;
mod tag;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub use catalog::{Arg, Catalog};
pub use load::{LANGUAGE_FILE, Loaded, load, read_json, read_language};
pub use message::{
    KINDS, NUMBERS_SCHEMA, SCHEMA, Segment, check_file, kind_of, names, parse, source_hash,
    validate, validate_source,
};
pub use plural::{PluralRules, compile_rules, plural_category};
pub use reason::Reason;
pub use tag::{chain, normalize_tag};

/// A connector's embedded English catalog file, parsed and checked once.
#[derive(Debug, Clone)]
pub struct Embedded {
    doc: Value,
    messages: Map<String, Value>,
    rev: String,
}

impl Embedded {
    /// Parses the catalog file text. `Err` (with the reason) when it is not JSON, its `schema`
    /// is not [`SCHEMA`], its `catalog` is not `catalog_id` or it has no `messages` object.
    pub fn parse(json: &str, catalog_id: &str) -> Result<Self, String> {
        let doc: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
        if doc.get("schema").and_then(Value::as_str) != Some(SCHEMA) {
            return Err(format!("schema is not {SCHEMA}"));
        }
        match doc.get("catalog").and_then(Value::as_str) {
            Some(id) if id == catalog_id => {}
            other => return Err(format!("catalog is {other:?}, expected {catalog_id:?}")),
        }
        let Some(Value::Object(messages)) = doc.get("messages") else {
            return Err("no messages object".to_string());
        };
        let messages = messages.clone();
        let canon = serde_json::to_string(&message::sorted(&doc)).unwrap_or_default();
        let rev = hex::encode(Sha256::digest(canon.as_bytes()))[..12].to_string();
        Ok(Self { doc, messages, rev })
    }

    /// The English source messages.
    pub fn messages(&self) -> &Map<String, Value> {
        &self.messages
    }

    /// `control.catalog.get`: the catalog file and its `rev`, the first 12 hex digits of the
    /// SHA-256 of its canonical JSON (sorted keys, no spaces, UTF-8 as it is).
    pub fn document(&self) -> (&Value, &str) {
        (&self.doc, &self.rev)
    }
}

/// The pseudo-locale pack of `messages` for tests: every text wrapped in `⟦…⟧` with accented
/// letters (placeholders untouched), so a test can find a visible text that did not go
/// through the catalog. Load it as the pack of the tag [`PSEUDO_TAG`].
pub fn pseudo_pack(messages: &Map<String, Value>) -> Map<String, Value> {
    messages
        .iter()
        .map(|(key, msg)| {
            let text = match msg.get("text") {
                Some(Value::Object(forms)) => Value::Object(
                    forms
                        .iter()
                        .map(|(c, v)| (c.clone(), Value::String(pseudo(v.as_str().unwrap_or("")))))
                        .collect(),
                ),
                other => Value::String(pseudo(other.and_then(Value::as_str).unwrap_or(""))),
            };
            (key.clone(), serde_json::json!({ "text": text }))
        })
        .collect()
}

/// The tag of the pseudo-locale.
pub const PSEUDO_TAG: &str = "qps";

fn accent(c: char) -> char {
    match c {
        'a' => 'á',
        'e' => 'é',
        'i' => 'í',
        'o' => 'ó',
        'u' => 'ú',
        'A' => 'Á',
        'E' => 'É',
        'I' => 'Í',
        'O' => 'Ó',
        'U' => 'Ú',
        'c' => 'ç',
        'n' => 'ñ',
        'y' => 'ý',
        'C' => 'Ç',
        'N' => 'Ñ',
        'Y' => 'Ý',
        other => other,
    }
}

fn pseudo(text: &str) -> String {
    let mut out = String::from("⟦");
    for seg in parse(text).unwrap_or_default() {
        match seg {
            Segment::Name(n) => {
                out.push('{');
                out.push_str(&n);
                out.push('}');
            }
            Segment::Lit(s) => {
                for c in s.chars() {
                    match c {
                        '{' => out.push_str("{{"),
                        '}' => out.push_str("}}"),
                        c => out.push(accent(c)),
                    }
                }
            }
        }
    }
    out.push('⟧');
    out
}

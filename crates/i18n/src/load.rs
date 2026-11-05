//! Loading a language from the workspace: `<workspace>/locales/<tag>/<catalog id>.json`
//! packs along the chain, over the embedded English (connector kit, catalogs.md sections 6 and 9).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::catalog::Catalog;
use crate::message::{NUMBERS_SCHEMA, check_file};
use crate::plural::{PluralRules, rules_from_value, valid_rules};
use crate::reason::Reason;
use crate::tag::{chain, normalize_tag};

/// The language setting in the workspace (the `language` field), relative to the workspace.
pub const LANGUAGE_FILE: [&str; 2] = ["hub", "settings.json"];

/// A catalog and what went wrong while loading it (for the caller to log once).
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The resolved catalog (its `rejected` holds the dropped entries).
    pub catalog: Catalog,
    /// Pack files skipped as a whole, with the reason.
    pub skipped: Vec<(PathBuf, Reason)>,
}

/// Reads a JSON file (a leading BOM is ignored). `Ok(None)` when the file does not exist.
pub fn read_json(path: &Path) -> Result<Option<Value>, Reason> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Reason::Unreadable),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| Reason::NotJson)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    serde_json::from_str(text)
        .map(Some)
        .map_err(|_| Reason::NotJson)
}

/// Folder tag in lower case to its folder in `<workspace>\locales`.
fn pack_dirs(workspace: &Path) -> BTreeMap<String, PathBuf> {
    let Ok(entries) = std::fs::read_dir(workspace.join("locales")) else {
        return BTreeMap::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
        .collect()
}

/// The plural rules of a folder's `numbers.json`, when it is a valid one for that folder.
fn folder_rules(folder: &Path) -> Option<PluralRules> {
    let doc = read_json(&folder.join("numbers.json")).ok()??;
    if doc.get("schema").and_then(Value::as_str) != Some(NUMBERS_SCHEMA) {
        return None;
    }
    let language = normalize_tag(doc.get("language")?.as_str()?)?;
    if language.to_lowercase() != folder.file_name()?.to_string_lossy().to_lowercase() {
        return None;
    }
    rules_from_value(doc.get("plural_rules")?).filter(valid_rules)
}

/// `language` of `<workspace>\hub\settings.json`; `en` when there is no workspace, file or
/// valid tag.
pub fn read_language(workspace: Option<&Path>) -> String {
    let Some(ws) = workspace else {
        return "en".to_string();
    };
    let path = LANGUAGE_FILE
        .iter()
        .fold(ws.to_path_buf(), |p, part| p.join(part));
    read_json(&path)
        .ok()
        .flatten()
        .and_then(|doc| normalize_tag(doc.get("language")?.as_str()?))
        .unwrap_or_else(|| "en".to_string())
}

/// The catalog `catalog_id` for `language`: `builtin` (the English source messages) and the
/// workspace's
/// packs along the chain, with the plural rules of the first folder of the chain that has a
/// valid `numbers.json` (English otherwise). Skipped files are in [`Loaded::skipped`],
/// rejected entries in `catalog.rejected`.
pub fn load(
    workspace: Option<&Path>,
    language: &str,
    catalog_id: &str,
    builtin: &Map<String, Value>,
) -> Loaded {
    let language = normalize_tag(language).unwrap_or_else(|| "en".to_string());
    let dirs = workspace.map(pack_dirs).unwrap_or_default();
    let mut packs: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut rules: Option<PluralRules> = None;
    for tag in chain(&language) {
        let Some(folder) = dirs.get(&tag.to_lowercase()) else {
            continue;
        };
        let path = folder.join(format!("{catalog_id}.json"));
        let name = folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match read_json(&path) {
            Ok(None) => {}
            Ok(Some(doc)) => match check_file(&doc, &name, catalog_id) {
                Ok(()) => {
                    if let Some(Value::Object(m)) = doc.get("messages") {
                        packs.insert(tag.clone(), m.clone());
                    }
                }
                Err(why) => skipped.push((path, why)),
            },
            Err(why) => skipped.push((path, why)),
        }
        if rules.is_none() {
            rules = folder_rules(folder);
        }
    }
    Loaded {
        catalog: Catalog::new(builtin, &language, &packs, rules),
        skipped,
    }
}

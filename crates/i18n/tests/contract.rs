//! The contract's test vectors (`tests/fixtures/catalogs.vectors.json`, a copy of the Stintmate
//! contract's `catalogs.vectors.json`) and a test connector's embedded English catalog.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use stintmate_connector_i18n::plural::{PluralRules, plural_category, rules_from_value};
use stintmate_connector_i18n::{
    Arg, Catalog, Embedded, Reason, chain, check_file, load, normalize_tag, read_json, source_hash,
    validate,
};

/// The catalog id of the test connector (its file is `tests/fixtures/test-connector.json`).
const CATALOG: &str = "test-connector";
const EN_JSON: &str = include_str!("fixtures/test-connector.json");

fn embedded() -> &'static Embedded {
    static E: OnceLock<Embedded> = OnceLock::new();
    E.get_or_init(|| Embedded::parse(EN_JSON, CATALOG).unwrap())
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn vectors() -> Value {
    let raw = fs::read_to_string(root().join("tests/fixtures/catalogs.vectors.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn list<'a>(v: &'a Value, section: &str) -> &'a Vec<Value> {
    v[section].as_array().unwrap()
}

fn rules(v: &Value) -> PluralRules {
    rules_from_value(v).unwrap()
}

fn arg(v: &Value) -> Arg {
    match v.as_i64() {
        Some(n) => Arg::Int(n),
        None => Arg::Text(v.as_str().unwrap().to_string()),
    }
}

fn args(params: &Value) -> Vec<(String, Arg)> {
    params
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), arg(v)))
        .collect()
}

fn text(cat: &Catalog, key: &str, params: &[(String, Arg)]) -> String {
    let borrowed: Vec<(&str, Arg)> = params
        .iter()
        .map(|(k, a)| (k.as_str(), a.clone()))
        .collect();
    cat.text(key, &borrowed)
}

#[test]
fn the_vectors_have_every_section() {
    let v = vectors();
    for s in [
        "tags",
        "plural",
        "hash",
        "formatting",
        "validation",
        "files",
        "resolve",
    ] {
        assert!(v.get(s).is_some(), "{s}");
    }
}

#[test]
fn tags() {
    let v = vectors();
    for case in list(&v, "tags") {
        let tag = case["tag"].as_str().unwrap_or(""); // null and 12: not a string, so not a tag
        assert_eq!(
            normalize_tag(tag).as_deref(),
            case["normalized"].as_str(),
            "{tag:?}"
        );
        let want: Vec<&str> = case["chain"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert_eq!(chain(tag), want, "{tag:?}");
    }
}

#[test]
fn plural_rules() {
    let v = vectors();
    for group in list(&v, "plural") {
        let r = rules(&group["rules"]);
        for case in group["cases"].as_array().unwrap() {
            let n = case["n"].as_i64().unwrap();
            assert_eq!(
                plural_category(&r, n),
                case["category"].as_str().unwrap(),
                "{r:?} {n}"
            );
        }
    }
}

#[test]
fn source_hashes() {
    let v = vectors();
    for case in list(&v, "hash") {
        assert_eq!(
            source_hash(&case["message"]),
            case["hash"].as_str().unwrap()
        );
    }
}

#[test]
fn the_hash_ignores_key_order_and_empty_params() {
    let a: Value = serde_json::from_str(r#"{"text":"x","params":{"b":"int","a":"text"}}"#).unwrap();
    let b: Value = serde_json::from_str(r#"{"params":{"a":"text","b":"int"},"text":"x"}"#).unwrap();
    assert_eq!(source_hash(&a), source_hash(&b));
    assert_eq!(
        source_hash(&json!({"text": "x"})),
        source_hash(&json!({"text": "x", "params": {}}))
    );
}

#[test]
fn formatting() {
    let v = vectors();
    for case in list(&v, "formatting") {
        let builtin = Map::from_iter([("x.case".to_string(), case["message"].clone())]);
        let cat = Catalog::english(&builtin);
        assert!(cat.invalid.is_empty(), "{:?}", cat.invalid);
        let got = text(&cat, "x.case", &args(&case["params"]));
        assert_eq!(got, case["expected"].as_str().unwrap());
    }
}

#[test]
fn validation() {
    let v = vectors();
    for case in list(&v, "validation") {
        let key = case["key"].as_str().unwrap();
        let source = (!case["source"].is_null()).then_some(&case["source"]);
        let got = validate(
            key,
            source,
            &case["translation"],
            &rules(&case["plural_rules"]),
        );
        let want = case["result"].as_str().unwrap();
        assert_eq!(got.err().map_or("ok", Reason::as_str), want, "{case}");
        if got.is_ok() {
            let stale = case["translation"]
                .get("source_hash")
                .and_then(Value::as_str)
                != Some(&source_hash(&case["source"]));
            assert_eq!(stale, case["stale"].as_bool().unwrap(), "{case}");
        }
    }
}

#[test]
fn files() {
    let v = vectors();
    for case in list(&v, "files") {
        let tmp = tempfile::tempdir().unwrap();
        let folder_name = case["folder"].as_str().unwrap();
        let catalog = case["catalog"].as_str().unwrap();
        let folder = tmp.path().join("locales").join(folder_name);
        fs::create_dir_all(&folder).unwrap();
        let raw = match case.get("raw") {
            Some(r) => r.as_str().unwrap().to_string(),
            None => case["content"].to_string(),
        };
        let path = folder.join(format!("{catalog}.json"));
        fs::write(&path, raw).unwrap();
        let got = read_json(&path).and_then(|doc| check_file(&doc.unwrap(), folder_name, catalog));
        assert_eq!(
            got.err().map_or("ok", Reason::as_str),
            case["result"].as_str().unwrap(),
            "{case}"
        );
    }
}

fn write_pack(ws: &Path, tag: &str, catalog: &str, language: &str, messages: &Value) {
    let folder = ws.join("locales").join(tag);
    fs::create_dir_all(&folder).unwrap();
    let doc = json!({"schema": "stintmate-catalog/1", "catalog": catalog, "language": language, "messages": messages});
    fs::write(folder.join(format!("{catalog}.json")), doc.to_string()).unwrap();
}

fn write_rules(ws: &Path, tag: &str, rules: &Value) {
    let folder = ws.join("locales").join(tag);
    fs::create_dir_all(&folder).unwrap();
    let doc = json!({"schema": "stintmate-numbers/1", "language": tag, "plural_rules": rules});
    fs::write(folder.join("numbers.json"), doc.to_string()).unwrap();
}

#[test]
fn resolve() {
    let v = vectors();
    let case = &v["resolve"];
    let tmp = tempfile::tempdir().unwrap();
    for (tag, messages) in case["packs"].as_object().unwrap() {
        write_pack(tmp.path(), tag, CATALOG, tag, messages);
    }
    for (tag, r) in case["plural_rules"].as_object().unwrap() {
        write_rules(tmp.path(), tag, r);
    }
    let builtin = case["builtin"].as_object().unwrap();
    let loaded = load(
        Some(tmp.path()),
        case["language"].as_str().unwrap(),
        CATALOG,
        builtin,
    );
    assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
    let cat = loaded.catalog;
    for (key, want) in case["expected"].as_object().unwrap() {
        assert_eq!(cat.text(key, &[]), want.as_str().unwrap(), "{key}");
    }
    for c in case["expected_plural"].as_array().unwrap() {
        let got = cat.text(c["key"].as_str().unwrap(), &[("n", arg(&c["n"]))]);
        assert_eq!(got, c["text"].as_str().unwrap());
    }
    let rejected: Value = cat
        .rejected
        .iter()
        .map(|(tag, m)| {
            (
                tag.clone(),
                m.iter()
                    .map(|(k, r)| (k.clone(), json!(r.as_str())))
                    .collect::<Value>(),
            )
        })
        .collect();
    assert_eq!(rejected, case["rejected"]);
}

// ---------------------------------------------------------------- the embedded catalog

/// sha256 of the test connector's catalog file; a change to it updates this pin.
const EN_JSON_SHA256: &str = "2a912b868c59c6b89a84332e0739b4d1ae76502c362546a6f8958ebcd0117bf4";

#[test]
fn the_embedded_catalog_is_the_pinned_file_byte_for_byte() {
    let hash = hex::encode(Sha256::digest(EN_JSON.as_bytes()));
    assert_eq!(hash, EN_JSON_SHA256);
    let on_disk = fs::read(root().join("tests/fixtures/test-connector.json")).unwrap();
    assert_eq!(hex::encode(Sha256::digest(on_disk)), EN_JSON_SHA256);
}

#[test]
fn the_fixture_is_the_contract_copy() {
    let raw = fs::read(root().join("tests/fixtures/catalogs.vectors.json")).unwrap();
    assert_eq!(
        hex::encode(Sha256::digest(raw)),
        "036a715b0c689af77eba62ac0451bbd922822c19fefabc3f3247f4e2e8b0513a"
    );
}

#[test]
fn the_embedded_header() {
    let doc = embedded().document().0;
    assert_eq!(doc["schema"], "stintmate-catalog/1");
    assert_eq!(doc["catalog"], CATALOG);
    assert_eq!(doc["language"], "en");
    assert_eq!(doc["language_name"], "English");
    let (_, rev) = embedded().document();
    assert_eq!(rev.len(), 12);
}

#[test]
fn every_embedded_message_follows_the_contract() {
    let en = PluralRules::from([("one".to_string(), "i = 1 and v = 0".to_string())]);
    let messages = embedded().messages();
    assert!(messages.len() > 40);
    for (key, msg) in messages {
        assert!(key.contains('.') && !key.starts_with("speech."), "{key}");
        assert_eq!(
            stintmate_connector_i18n::validate_source(key, msg),
            Ok(()),
            "{key}"
        );
        let params = msg.get("params").and_then(Value::as_object);
        for spec in params.into_iter().flatten().map(|(_, s)| s) {
            assert!(
                matches!(
                    stintmate_connector_i18n::kind_of(Some(spec)),
                    Some("text" | "int" | "enum")
                ),
                "{key}"
            );
        }
        if let Some(plural) = msg.get("plural").and_then(Value::as_str) {
            assert_eq!(
                stintmate_connector_i18n::kind_of(params.unwrap().get(plural)),
                Some("int"),
                "{key}"
            );
            let cats: Vec<&String> = msg["text"].as_object().unwrap().keys().collect();
            assert!(
                cats.iter().all(|c| *c == "one" || *c == "other")
                    && cats.iter().any(|c| *c == "other"),
                "{key}"
            );
        }
        // The source passes the rules a translation of it must pass.
        assert_eq!(validate(key, Some(msg), msg, &en), Ok(()), "{key}");
    }
    assert!(Catalog::english(messages).invalid.is_empty());
}

#[test]
fn the_embedded_english_texts() {
    let cat = Catalog::english(embedded().messages());
    assert_eq!(cat.text("service.recorder", &[]), "Recorder");
    assert_eq!(
        cat.text("detail.outbox", &[("n", 1.into())]),
        "1 lap waiting for Pitwall"
    );
    assert_eq!(
        cat.text("detail.outbox", &[("n", 0.into())]),
        "0 laps waiting for Pitwall"
    );
    assert_eq!(
        cat.text("logs.copy", &[("n", 1234.into())]),
        "Copy last 1234"
    );
    assert_eq!(cat.text("no.such_key", &[]), "no.such_key");
    assert!(!cat.contains("no.such_key"));
}

// ---------------------------------------------------------------- loading a language

fn es_rules() -> Value {
    json!({"one": "n = 1", "many": "e = 0 and i != 0 and i % 1000000 = 0 and v = 0 or e != 0..5"})
}

#[test]
fn language_setting() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(stintmate_connector_i18n::read_language(None), "en");
    assert_eq!(
        stintmate_connector_i18n::read_language(Some(tmp.path())),
        "en"
    );
    fs::create_dir_all(tmp.path().join("hub")).unwrap();
    let settings = tmp.path().join("hub/settings.json");
    fs::write(&settings, r#"{"schema": 1, "language": "es-cl"}"#).unwrap();
    assert_eq!(
        stintmate_connector_i18n::read_language(Some(tmp.path())),
        "es-CL"
    );
    fs::write(&settings, r#"{"schema": 1, "language": "Spanish"}"#).unwrap();
    assert_eq!(
        stintmate_connector_i18n::read_language(Some(tmp.path())),
        "en"
    );
    fs::write(&settings, "[not json").unwrap();
    assert_eq!(
        stintmate_connector_i18n::read_language(Some(tmp.path())),
        "en"
    );
}

#[test]
fn a_pack_along_the_chain_with_per_key_fallback() {
    let messages = embedded().messages();
    let tmp = tempfile::tempdir().unwrap();
    let hash = |k: &str| source_hash(&messages[k]);
    write_pack(
        tmp.path(),
        "es",
        CATALOG,
        "es",
        &json!({
            "service.recorder": {"text": "Grabador", "source_hash": hash("service.recorder")},
            "tray.exit": {"text": "Salir"},
            "detail.outbox": {"text": {"one": "{n} vuelta", "many": "{n} de vueltas", "other": "{n} vueltas"},
                              "source_hash": hash("detail.outbox")},
            "status.recorder.lap": {"text": "vuelta {vuelta}"},
            "no.such_key": {"text": "x"},
        }),
    );
    write_pack(
        tmp.path(),
        "es-CL",
        CATALOG,
        "es-CL",
        &json!({"service.recorder": {"text": "Grabadora"}}),
    );
    write_rules(tmp.path(), "es", &es_rules());
    let loaded = load(Some(tmp.path()), "es-CL", CATALOG, messages);
    assert!(loaded.skipped.is_empty());
    let cat = loaded.catalog;
    assert_eq!(cat.language(), "es-CL");
    assert_eq!(cat.text("service.recorder", &[]), "Grabadora");
    assert_eq!(cat.text("tray.exit", &[]), "Salir");
    assert_eq!(cat.text("page.logs", &[]), "Logs");
    assert_eq!(
        cat.text("status.recorder.lap", &[("lap", 3.into())]),
        "lap 3"
    );
    assert_eq!(cat.text("detail.outbox", &[("n", 1.into())]), "1 vuelta");
    assert_eq!(cat.text("detail.outbox", &[("n", 2.into())]), "2 vueltas");
    assert_eq!(
        cat.text("detail.outbox", &[("n", 1_000_000.into())]),
        "1000000 de vueltas"
    );
    let rejected: Vec<(&str, &str, &str)> = cat
        .rejected
        .iter()
        .flat_map(|(t, m)| {
            m.iter()
                .map(move |(k, r)| (t.as_str(), k.as_str(), r.as_str()))
        })
        .collect();
    assert_eq!(
        rejected,
        [
            ("es", "no.such_key", "unknown_key"),
            ("es", "status.recorder.lap", "placeholder_mismatch")
        ]
    );
    let stale: Vec<&str> = cat.stale.iter().map(String::as_str).collect();
    assert_eq!(stale, ["service.recorder", "tray.exit"]); // es-CL's entry has no hash
}

#[test]
fn a_broken_pack_file_is_skipped_whole() {
    let messages = embedded().messages();
    let tmp = tempfile::tempdir().unwrap();
    let folder = tmp.path().join("locales/es");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("test-connector.json"), "{broken").unwrap();
    let loaded = load(Some(tmp.path()), "es", CATALOG, messages);
    assert_eq!(loaded.catalog.text("service.recorder", &[]), "Recorder");
    assert_eq!(loaded.skipped.len(), 1);
    assert_eq!(loaded.skipped[0].1, Reason::NotJson);
    write_pack(
        tmp.path(),
        "es",
        "hub",
        "es",
        &json!({"service.recorder": {"text": "Grabador"}}),
    );
    fs::rename(folder.join("hub.json"), folder.join("test-connector.json")).unwrap();
    let loaded = load(Some(tmp.path()), "es", CATALOG, messages);
    assert_eq!(loaded.catalog.text("service.recorder", &[]), "Recorder");
    assert_eq!(loaded.skipped[0].1, Reason::CatalogMismatch);
}

#[test]
fn folder_names_ignore_case() {
    let messages = embedded().messages();
    let tmp = tempfile::tempdir().unwrap();
    write_pack(
        tmp.path(),
        "ES-cl",
        CATALOG,
        "es-CL",
        &json!({"page.logs": {"text": "Registros"}}),
    );
    let loaded = load(Some(tmp.path()), "es-cl", CATALOG, messages);
    assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
    assert_eq!(loaded.catalog.text("page.logs", &[]), "Registros");
}

#[test]
fn an_invalid_numbers_file_leaves_the_english_rules() {
    let messages = embedded().messages();
    let tmp = tempfile::tempdir().unwrap();
    write_rules(tmp.path(), "es", &json!({"one": "n ~ 1"}));
    let cat = load(Some(tmp.path()), "es", CATALOG, messages).catalog;
    assert_eq!(
        cat.rules(),
        &stintmate_connector_i18n::plural::english_rules()
    );
}

#[test]
fn enum_values_come_from_the_source_and_the_pack() {
    let builtin = json!({"x.wheel": {"text": "Puncture, {wheel}.",
        "params": {"wheel": {"kind": "enum", "values": ["fl", "fr"]}},
        "values": {"wheel": {"fl": "front left", "fr": "front right"}}}});
    let source = builtin.as_object().unwrap();
    let cat = Catalog::english(source);
    assert_eq!(
        cat.text("x.wheel", &[("wheel", "fr".into())]),
        "Puncture, front right."
    );
    assert_eq!(
        cat.text("x.wheel", &[("wheel", "rl".into())]),
        "Puncture, rl."
    );
    let pack = json!({"x.wheel": {"text": "Pinchazo, {wheel}.",
        "values": {"wheel": {"fl": "delantera izquierda", "fr": "delantera derecha"}}}});
    let packs =
        std::collections::BTreeMap::from([("es".to_string(), pack.as_object().unwrap().clone())]);
    let cat = Catalog::new(source, "es", &packs, None);
    assert!(cat.rejected.is_empty(), "{:?}", cat.rejected);
    assert_eq!(
        cat.text("x.wheel", &[("wheel", "fl".into())]),
        "Pinchazo, delantera izquierda."
    );
}

// ---------------------------------------------------------------- the pseudo-locale

#[test]
fn pseudo_locale_every_text_goes_through_the_catalog() {
    let messages = embedded().messages();
    let pack = stintmate_connector_i18n::pseudo_pack(messages);
    let packs = std::collections::BTreeMap::from([(
        stintmate_connector_i18n::PSEUDO_TAG.to_string(),
        pack,
    )]);
    let cat = Catalog::new(messages, stintmate_connector_i18n::PSEUDO_TAG, &packs, None);
    assert!(cat.rejected.is_empty(), "{:?}", cat.rejected);
    assert_eq!(cat.translated.len(), messages.len());
    assert_eq!(cat.text("service.recorder", &[]), "⟦Réçórdér⟧");
    for (key, msg) in messages {
        let params: Vec<(String, Arg)> = msg
            .get("params")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(p, s)| {
                let a = match stintmate_connector_i18n::kind_of(Some(s)) {
                    Some("int") => Arg::Int(2),
                    _ => Arg::Text("v".into()),
                };
                (p.clone(), a)
            })
            .collect();
        let shown = text(&cat, key, &params);
        assert!(
            shown.starts_with('⟦') && shown.ends_with('⟧'),
            "{key}: {shown}"
        );
    }
}

#[test]
fn embedded_rejects_wrong_catalog_id() {
    assert!(Embedded::parse(EN_JSON, "other-connector").is_err());
    assert!(Embedded::parse("{broken", CATALOG).is_err());
    assert!(Embedded::parse(r#"{"schema":"x","catalog":"test-connector"}"#, CATALOG).is_err());
    assert!(
        Embedded::parse(
            r#"{"schema":"stintmate-catalog/1","catalog":"test-connector"}"#,
            CATALOG
        )
        .is_err()
    );
}

#[test]
fn load_reads_the_catalog_id_file() {
    let tmp = tempfile::tempdir().unwrap();
    let messages = embedded().messages();
    write_pack(
        tmp.path(),
        "es",
        "test-connector",
        "es",
        &json!({"service.recorder": {"text": "Grabador"}}),
    );
    let loaded = load(Some(tmp.path()), "es", "test-connector", messages);
    assert_eq!(loaded.catalog.text("service.recorder", &[]), "Grabador");
    // The same folder under another catalog id holds no pack for it.
    let loaded = load(Some(tmp.path()), "es", "another-connector", messages);
    assert_eq!(loaded.catalog.text("service.recorder", &[]), "Recorder");
}

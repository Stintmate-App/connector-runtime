//! The control app's texts: the connector's embedded English catalog with the workspace's packs
//! over it.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use stintmate_connector_i18n::{Arg, Catalog, Embedded};
use stintmate_connector_platform::spec::ConnectorSpec;

/// Every key the control app and the window use, with the placeholders each one takes. A
/// connector's English catalog must hold all of them; its own test calls [`check_ui_catalog`].
pub const REQUIRED_KEYS: &[(&str, &[&str])] = &[
    ("app.name", &[]),
    ("data.source.workspace", &["source", "workspace"]),
    ("detail.cpu", &["percent"]),
    ("detail.forced_stop", &[]),
    ("detail.game_not_running", &[]),
    ("detail.game_running", &[]),
    ("detail.outbox", &["n"]),
    ("detail.pid", &["pid"]),
    ("detail.rejected", &["n"]),
    ("exit.button", &[]),
    ("exit.tip", &[]),
    ("logs.alerts_only", &[]),
    ("logs.copy", &["n"]),
    ("logs.footer", &["file"]),
    ("logs.none_yet", &["file"]),
    ("logs.open", &[]),
    ("logs.ui", &[]),
    ("nav.name", &[]),
    ("nav.version", &["version"]),
    ("notice.crashed.title", &["service"]),
    ("notice.forced.message", &["seconds"]),
    ("notice.forced.title", &["service"]),
    ("notice.see_logs", &[]),
    ("notice.shm_error.message", &[]),
    ("notice.shm_error.title", &[]),
    ("notice.start_failed", &["service"]),
    ("options.autostart", &[]),
    ("options.autostart.note", &[]),
    ("options.error.autostart_change", &["error"]),
    ("options.error.autostart_read", &["error"]),
    ("options.error.save", &["error"]),
    ("options.hz.advanced", &[]),
    ("options.hz.name", &[]),
    ("options.hz.note", &[]),
    ("options.hz.suffix", &[]),
    ("options.minimized", &[]),
    ("options.minimized.note", &[]),
    ("options.start", &["service"]),
    ("options.start.note", &[]),
    ("options.start.recorder.note", &[]),
    ("page.logs", &[]),
    ("page.options", &[]),
    ("page.services", &[]),
    ("service.recorder", &[]),
    ("services.data_folder", &[]),
    ("services.managed", &[]),
    ("services.open", &[]),
    ("services.open_data_folder", &[]),
    ("services.stopping", &[]),
    ("shortcut.needs_stintmate.text", &[]),
    ("startup.error.detail", &["code", "message"]),
    ("startup.error.hint", &[]),
    ("startup.error.title", &[]),
    ("status.crashed", &[]),
    ("status.game_waiting", &[]),
    ("status.garage", &[]),
    ("status.menus", &[]),
    ("status.not_installed", &[]),
    ("status.off", &[]),
    ("status.ownership.adopted", &[]),
    ("status.ownership.external", &[]),
    ("status.ownership.untied", &[]),
    ("status.pitwall.absent", &[]),
    ("status.pitwall.connected", &[]),
    ("status.pitwall.refused", &[]),
    ("status.recorder.driving_offline", &[]),
    ("status.recorder.garage_offline", &[]),
    ("status.recorder.lap", &["lap"]),
    ("status.recorder.off_game_running", &[]),
    ("status.recorder.recording", &[]),
    ("status.recorder.session", &["session"]),
    ("status.recorder.stint", &["stint"]),
    ("status.shm_error", &[]),
    ("status.starting", &[]),
    ("status.stopping", &[]),
    ("status.tooltip_line", &["service", "state"]),
    ("switch.off", &[]),
    ("switch.on", &[]),
    ("tile.cpu", &[]),
    ("tile.pitwall", &[]),
    ("tile.pitwall.absent", &[]),
    ("tile.pitwall.connected", &[]),
    ("tile.pitwall.refused", &[]),
    ("tile.rate", &[]),
    ("tray.exit", &[]),
    ("tray.logs", &[]),
    ("tray.open", &[]),
    ("tray.service.start", &["service"]),
    ("tray.service.stop", &["service"]),
    ("tray.stopping", &[]),
];

/// Checks a connector's English UI catalog text against [`REQUIRED_KEYS`]: the file must parse
/// as a catalog with id `catalog_id`, every required key must be there and its text must use
/// exactly the placeholders listed. Returns the problems found, empty when the catalog is good.
pub fn check_ui_catalog(json: &str, catalog_id: &str) -> Vec<String> {
    let embedded = match Embedded::parse(json, catalog_id) {
        Ok(e) => e,
        Err(why) => return vec![format!("invalid catalog file: {why}")],
    };
    let messages = embedded.messages();
    let mut problems = Vec::new();
    for (key, wanted) in REQUIRED_KEYS {
        let Some(msg) = messages.get(*key) else {
            problems.push(format!("missing key {key}"));
            continue;
        };
        let texts: Vec<&str> = match msg.get("text") {
            Some(serde_json::Value::String(s)) => vec![s.as_str()],
            Some(serde_json::Value::Object(forms)) => {
                forms.values().filter_map(|v| v.as_str()).collect()
            }
            _ => Vec::new(),
        };
        if texts.is_empty() {
            problems.push(format!("key {key} has no text"));
            continue;
        }
        let wanted: BTreeSet<String> = wanted.iter().map(|s| (*s).to_string()).collect();
        for text in texts {
            match stintmate_connector_i18n::parse(text) {
                Ok(segments) => {
                    let found = stintmate_connector_i18n::names(&segments);
                    if found != wanted {
                        problems.push(format!(
                            "key {key} has placeholders {found:?}, expected {wanted:?}"
                        ));
                        break;
                    }
                }
                Err(why) => {
                    problems.push(format!("key {key} has a bad text: {why:?}"));
                    break;
                }
            }
        }
    }
    problems
}
/// A resolved catalog, cheap to clone.
#[derive(Clone)]
pub struct Texts {
    spec: &'static ConnectorSpec,
    catalog: Arc<Catalog>,
    embedded: Arc<Embedded>,
}

/// The embedded English catalog of `spec`. A catalog that does not parse is a build error of the
/// connector, caught by its `check_ui_catalog` test; at run time it is an empty one.
pub fn embedded(spec: &ConnectorSpec) -> Embedded {
    Embedded::parse(spec.ui_catalog, &spec.catalog_id()).unwrap_or_else(|why| {
        tracing::error!("The embedded UI catalog is invalid: {why}");
        Embedded::parse(
            &format!(
                r#"{{"schema":"{}","catalog":"{}","messages":{{}}}}"#,
                stintmate_connector_i18n::SCHEMA,
                spec.catalog_id()
            ),
            &spec.catalog_id(),
        )
        .expect("an empty catalog is valid")
    })
}

impl Texts {
    /// The embedded English texts of `spec`.
    pub fn english(spec: &'static ConnectorSpec) -> Self {
        let embedded = embedded(spec);
        Self {
            spec,
            catalog: Arc::new(Catalog::english(embedded.messages())),
            embedded: Arc::new(embedded),
        }
    }

    /// The language of `<workspace>\hub\settings.json` with the workspace's packs.
    pub fn load(spec: &'static ConnectorSpec, workspace: Option<&Path>) -> Self {
        let embedded = embedded(spec);
        let language = stintmate_connector_i18n::read_language(workspace);
        let loaded = stintmate_connector_i18n::load(
            workspace,
            &language,
            &spec.catalog_id(),
            embedded.messages(),
        );
        for (path, why) in &loaded.skipped {
            tracing::warn!("Language pack {} skipped: {why:?}", path.display());
        }
        Self {
            spec,
            catalog: Arc::new(loaded.catalog),
            embedded: Arc::new(embedded),
        }
    }

    /// The connector these texts belong to.
    pub fn spec(&self) -> &'static ConnectorSpec {
        self.spec
    }

    /// The embedded catalog file and its `rev` (`control.catalog.get`).
    pub fn document(&self) -> (&serde_json::Value, &str) {
        self.embedded.document()
    }

    /// The text of `key` with its placeholders filled.
    pub fn t(&self, key: &str, params: &[(&str, Arg)]) -> String {
        self.catalog.text(key, params)
    }

    /// The text of a key with no placeholders.
    pub fn s(&self, key: &str) -> String {
        self.catalog.text(key, &[])
    }

    /// The built-in English text of a plain message (for files that stay English).
    pub fn english_text(spec: &ConnectorSpec, key: &str) -> String {
        embedded(spec)
            .messages()
            .get(key)
            .and_then(|m| m.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or(key)
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testspec::SPEC;

    #[test]
    fn the_example_catalog_is_the_specs_and_names_the_connector() {
        let t = Texts::english(&SPEC);
        assert_eq!(t.s("app.name"), "Example connector");
        let (doc, rev) = t.document();
        assert_eq!(doc["catalog"], "example-connector");
        assert_eq!(rev.len(), 12);
        assert_eq!(Texts::english_text(&SPEC, "app.name"), "Example connector");
        assert_eq!(Texts::english_text(&SPEC, "no.such.key"), "no.such.key");
    }

    #[test]
    fn a_catalog_with_another_id_falls_back_to_an_empty_one() {
        static WRONG: ConnectorSpec = ConnectorSpec {
            id: "other",
            ..crate::testspec::SPEC
        };
        let t = Texts::english(&WRONG);
        assert_eq!(t.document().0["catalog"], "other-connector");
        assert_eq!(t.s("app.name"), "app.name");
    }

    #[test]
    fn load_without_a_workspace_is_english() {
        let t = Texts::load(&SPEC, None);
        assert_eq!(t.s("app.name"), "Example connector");
        assert_eq!(t.spec().id, "example");
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    use crate::testspec::{CATALOG, SPEC};

    #[test]
    fn example_catalog_has_every_required_key() {
        assert_eq!(
            check_ui_catalog(CATALOG, "example-connector"),
            Vec::<String>::new()
        );
        assert_eq!(CATALOG, SPEC.ui_catalog);
    }

    #[test]
    fn the_testing_crates_example_spec_carries_the_same_catalog() {
        assert_eq!(
            stintmate_connector_testing::EXAMPLE_SPEC.ui_catalog,
            CATALOG
        );
    }

    #[test]
    fn check_ui_catalog_reports_missing_keys_wrong_placeholders_and_bad_files() {
        let mut doc: serde_json::Value = serde_json::from_str(CATALOG).unwrap();
        doc["messages"].as_object_mut().unwrap().remove("app.name");
        doc["messages"]["detail.pid"]["text"] = "PID {other}".into();
        doc["messages"]["detail.pid"]["params"] = serde_json::json!({"other": "text"});
        let problems = check_ui_catalog(&doc.to_string(), "example-connector");
        assert!(problems.iter().any(|p| p == "missing key app.name"));
        assert!(problems.iter().any(|p| p.contains("detail.pid")));
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert_eq!(check_ui_catalog("nope", "example-connector").len(), 1);
        assert_eq!(check_ui_catalog(CATALOG, "other-connector").len(), 1);
    }
}

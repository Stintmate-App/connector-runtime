//! The control app's texts: the connector's embedded English catalog with the workspace's packs
//! over it.

use std::path::Path;
use std::sync::Arc;
use stintmate_connector_i18n::{Arg, Catalog, Embedded};
use stintmate_connector_platform::spec::ConnectorSpec;

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

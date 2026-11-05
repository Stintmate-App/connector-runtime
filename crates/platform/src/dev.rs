//! Development isolation: a second connector that shares nothing with the installed one.
//!
//! `STINTMATE_DEV_INSTANCE=<tag>` is read here and nowhere else (then each name in the spec's
//! `dev_env_aliases`, for a connector that had its own variable before). When set, the
//! single-instance names ([`crate::single::Names::for_spec`]) carry the tag and every pipe the
//! executables build uses the pipe user `<user>-<tag>`, so the connector's own endpoint and
//! the Pitwall pipe it dials are a separate set from the real ones. For development and desk
//! tests only; production never sets it.

use crate::spec::ConnectorSpec;

/// The environment variable.
pub const ENV: &str = "STINTMATE_DEV_INSTANCE";

/// Longest tag kept.
const MAX_TAG: usize = 24;

/// Normalises a raw tag: ASCII-lowercased, anything but `a-z0-9` becomes `-`, at most 24
/// characters; `None` when nothing is left (unset, empty or blank).
pub fn parse_tag(raw: Option<&str>) -> Option<String> {
    let t: String = raw?
        .trim()
        .chars()
        .take(MAX_TAG)
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let t = t.trim_matches('-').to_owned();
    (!t.is_empty()).then_some(t)
}

/// The development tag of this process: [`ENV`], else the first of the spec's
/// `dev_env_aliases` that is set to something usable.
pub fn tag(spec: &ConnectorSpec) -> Option<String> {
    tag_from(spec, |name| std::env::var(name).ok())
}

/// [`tag`] over an injectable environment lookup.
pub fn tag_from(spec: &ConnectorSpec, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    std::iter::once(ENV)
        .chain(spec.dev_env_aliases.iter().copied())
        .find_map(|name| parse_tag(get(name).as_deref()))
}

/// The pipe user for `user` under `tag`: `<user>-<tag>`.
pub fn tagged_user(user: &str, tag: &str) -> String {
    format!("{user}-{tag}")
}

/// `\\.\pipe\stintmate-<app>-<user>[-<tag>]` for an explicit user and tag.
pub fn pipe_path_with(
    app: &str,
    user: &str,
    tag: Option<&str>,
) -> Result<String, stintmate_ipc::error::NameError> {
    match tag {
        Some(t) => stintmate_ipc::naming::pipe_path_for(app, None, &tagged_user(user, t)),
        None => stintmate_ipc::naming::pipe_path_for(app, None, user),
    }
}

/// The pipe path of endpoint `app` for this process: the user's real one, or the tagged one
/// when a tag is set (see [`tag`]). Every pipe of a connector's executables is built here.
pub fn pipe_path(
    spec: &ConnectorSpec,
    app: &str,
) -> Result<String, stintmate_ipc::error::NameError> {
    match tag(spec) {
        None => stintmate_ipc::naming::pipe_path(app, None),
        Some(t) => pipe_path_with(app, &stintmate_ipc::naming::pipe_user()?, Some(&t)),
    }
}

/// The tagged pipe of `app` when a tag is set, else `None` (the library's own default then
/// applies).
pub fn pipe_path_if_dev(spec: &ConnectorSpec, app: &str) -> Option<String> {
    tag(spec)?;
    pipe_path(spec, app).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_normalised() {
        assert_eq!(parse_tag(None), None);
        assert_eq!(parse_tag(Some("")), None);
        assert_eq!(parse_tag(Some("  ")), None);
        assert_eq!(parse_tag(Some("--")), None);
        assert_eq!(parse_tag(Some("Desk1")).as_deref(), Some("desk1"));
        assert_eq!(parse_tag(Some(r"a b\c")).as_deref(), Some("a-b-c"));
        assert_eq!(parse_tag(Some(&"x".repeat(50))).unwrap().len(), MAX_TAG);
    }

    #[test]
    fn tagged_pipes_differ_from_the_real_ones() {
        let real = pipe_path_with("connector-example", "Alex", None).unwrap();
        let dev = pipe_path_with("connector-example", "Alex", Some("t1")).unwrap();
        assert_eq!(real, r"\\.\pipe\stintmate-connector-example-alex");
        assert_eq!(dev, r"\\.\pipe\stintmate-connector-example-alex-t1");
        let pw = pipe_path_with("pitwall", "Alex", Some("t1")).unwrap();
        assert_eq!(pw, r"\\.\pipe\stintmate-pitwall-alex-t1");
    }
}

#[cfg(test)]
mod env_tests {
    use super::*;
    use crate::spec::{ConnectorSpec, GameSpec};
    use stintmate_connector_resources::Glyph;

    const SPEC: ConnectorSpec = ConnectorSpec {
        id: "example",
        name: "Example connector",
        sim: "example",
        sim_name: "Example Simulator",
        product: "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a43",
        version: "0.1.0",
        exe_prefix: "example",
        mutex_prefix: "ExampleConnector",
        autostart_value: "Stintmate Example connector",
        game: GameSpec {
            exes: &["example-sim.exe"],
            window_title: None,
            window_class: None,
            cheap_hint: None,
        },
        recorder_hz: 60.0,
        features: &[],
        ui_catalog: "{}",
        glyph: Glyph::A,
        dev_env_aliases: &["EXAMPLE_OLD", "EXAMPLE_OLDER"],
    };

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn the_kit_variable_takes_precedence_over_an_alias() {
        let both = [("EXAMPLE_OLD", "old"), (ENV, "New 1")];
        assert_eq!(tag_from(&SPEC, env(&both)).as_deref(), Some("new-1"));
    }

    #[test]
    fn aliases_are_tried_in_order_and_blank_values_are_skipped() {
        let pairs = [(ENV, "  "), ("EXAMPLE_OLD", "--"), ("EXAMPLE_OLDER", "B")];
        assert_eq!(tag_from(&SPEC, env(&pairs)).as_deref(), Some("b"));
        assert_eq!(tag_from(&SPEC, env(&[])), None);
    }
}

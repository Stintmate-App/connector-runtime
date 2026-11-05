//! Every rule of `ConnectorSpec::validate`, and the derived names.

mod common;

use common::SPEC;
use stintmate_connector_platform::spec::{ConnectorSpec, FEATURES, GameSpec};

fn problems(spec: ConnectorSpec) -> Vec<String> {
    spec.validate().expect_err("must be refused")
}

#[test]
fn the_example_spec_is_valid_and_names_derive_from_it() {
    SPEC.validate().unwrap();
    assert_eq!(SPEC.endpoint(), "connector-example");
    assert_eq!(SPEC.ui_app_id(), "example-connector-ui");
    assert_eq!(SPEC.catalog_id(), "example-connector");
    assert_eq!(SPEC.recorder_exe(), "example-recorder.exe");
    assert_eq!(SPEC.window_exe(), "example-connector-window.exe");
}

#[test]
fn the_features_vocabulary_is_the_kits() {
    assert_eq!(FEATURES.len(), 20);
    let mut all = FEATURES.to_vec();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 20, "no duplicates");
    ConnectorSpec {
        features: &FEATURES,
        ..SPEC
    }
    .validate()
    .unwrap();
}

#[test]
fn the_id_must_be_a_short_lowercase_name() {
    for bad in [
        "",
        "Example",
        "-x",
        "_x",
        "a b",
        "a.b",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ] {
        let p = problems(ConnectorSpec { id: bad, ..SPEC });
        assert!(p.iter().any(|m| m.starts_with("id ")), "{bad:?}: {p:?}");
    }
    for good in [
        "a",
        "0",
        "iracing",
        "my_sim-2",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ] {
        ConnectorSpec { id: good, ..SPEC }.validate().unwrap();
    }
}

#[test]
fn the_product_must_be_a_lowercase_uuid_v4() {
    for bad in [
        "",
        "6F1C2A84-3B7E-4D59-9A0C-5E8D7B2F1A43",
        "6f1c2a84-3b7e-3d59-9a0c-5e8d7b2f1a43",
        "6f1c2a84-3b7e-4d59-7a0c-5e8d7b2f1a43",
        "6f1c2a843b7e4d599a0c5e8d7b2f1a43",
        "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a4",
        "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a4g",
    ] {
        let p = problems(ConnectorSpec {
            product: bad,
            ..SPEC
        });
        assert!(
            p.iter().any(|m| m.starts_with("product ")),
            "{bad:?}: {p:?}"
        );
    }
}

#[test]
fn the_prefixes_have_their_alphabets() {
    for bad in ["", "Example", "a_b", "a b"] {
        let p = problems(ConnectorSpec {
            exe_prefix: bad,
            ..SPEC
        });
        assert!(p.iter().any(|m| m.starts_with("exe_prefix ")), "{bad:?}");
    }
    for bad in ["", "Example-Sim", "a.b", "a b", "a_b"] {
        let p = problems(ConnectorSpec {
            mutex_prefix: bad,
            ..SPEC
        });
        assert!(p.iter().any(|m| m.starts_with("mutex_prefix ")), "{bad:?}");
    }
    ConnectorSpec {
        exe_prefix: "my-sim2",
        mutex_prefix: "MySim2Connector",
        ..SPEC
    }
    .validate()
    .unwrap();
}

#[test]
fn unknown_features_are_refused() {
    let p = problems(ConnectorSpec {
        features: &["live_frame", "teleport"],
        ..SPEC
    });
    assert_eq!(p.len(), 1);
    assert!(p[0].contains("teleport"));
}

#[test]
fn the_recorder_rate_is_bounded() {
    for bad in [0.0, 0.5, 200.5, f64::NAN, f64::INFINITY] {
        let p = problems(ConnectorSpec {
            recorder_hz: bad,
            ..SPEC
        });
        assert!(p.iter().any(|m| m.starts_with("recorder_hz")), "{bad}");
    }
    for good in [1.0, 60.0, 200.0] {
        ConnectorSpec {
            recorder_hz: good,
            ..SPEC
        }
        .validate()
        .unwrap();
    }
}

#[test]
fn the_game_needs_lowercase_exes_and_a_whole_window_description() {
    let game = |g: GameSpec| ConnectorSpec { game: g, ..SPEC };
    let base = SPEC.game;
    assert!(
        problems(game(GameSpec { exes: &[], ..base }))
            .iter()
            .any(|m| m.contains("empty"))
    );
    assert!(
        problems(game(GameSpec {
            exes: &["Sim.exe"],
            ..base
        }))
        .iter()
        .any(|m| m.contains("lowercase"))
    );
    assert!(
        problems(game(GameSpec {
            exes: &[""],
            ..base
        }))
        .iter()
        .any(|m| m.contains("lowercase"))
    );
    assert!(
        problems(game(GameSpec {
            window_class: None,
            ..base
        }))
        .iter()
        .any(|m| m.contains("together"))
    );
    assert!(
        problems(game(GameSpec {
            window_title: None,
            ..base
        }))
        .iter()
        .any(|m| m.contains("together"))
    );
    assert!(
        problems(game(GameSpec {
            window_title: Some("Example"),
            ..base
        }))
        .iter()
        .any(|m| m.contains("window_title"))
    );
    game(GameSpec {
        window_title: None,
        window_class: None,
        ..base
    })
    .validate()
    .unwrap();
}

#[test]
fn every_problem_is_reported_at_once() {
    let p = problems(ConnectorSpec {
        id: "X",
        product: "no",
        recorder_hz: 0.0,
        ..SPEC
    });
    assert_eq!(p.len(), 3, "{p:?}");
}

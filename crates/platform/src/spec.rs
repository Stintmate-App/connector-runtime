//! What a connector is: [`ConnectorSpec`], the one value through which everything that depends
//! on the simulator reaches the runtime, and its validation.
//!
//! A connector declares one `static` spec and passes it (as `&'static ConnectorSpec`) to the
//! runtime's entry points; there is no global. [`ConnectorSpec::validate`] checks the values the
//! rest of the runtime builds names, files and pipes from.

use stintmate_connector_resources::{ConnectorIcon, Glyph};

/// The lowest recorder rate a spec may default to, in Hz.
pub const HZ_MIN: f64 = 1.0;
/// The highest recorder rate a spec may default to, in Hz.
pub const HZ_MAX: f64 = 200.0;

/// The features a connector may declare in its descriptor (the connector kit's `contract.json`
/// vocabulary).
pub const FEATURES: [&str; 20] = [
    "live_frame",
    "lap_status",
    "capture_target",
    "brake_cues",
    "setups",
    "fuel",
    "sectors",
    "conditions",
    "rivals",
    "wheel_channels",
    "off_track",
    "announcements",
    "spotter",
    "guidance",
    "actions",
    "window",
    "notices",
    "translated_texts",
    "remembered_services",
    "options",
];

/// How the runtime recognises the simulator.
#[derive(Debug, Clone, Copy)]
pub struct GameSpec {
    /// The game's executable image names, lowercase.
    pub exes: &'static [&'static str],
    /// The game window's title, lowercase, matched exactly (after trimming).
    pub window_title: Option<&'static str>,
    /// The game window's class. Set together with `window_title`, or not at all.
    pub window_class: Option<&'static str>,
    /// A cheap "the game may run" signal, asked between full process snapshots.
    pub cheap_hint: Option<fn() -> bool>,
}

/// Everything that makes one connector different from another.
#[derive(Debug, Clone, Copy)]
pub struct ConnectorSpec {
    /// The connector's id, e.g. `iracing`: data folder, descriptor file, hello `connector_id`
    /// and idempotency-key prefix.
    pub id: &'static str,
    /// The display name, e.g. `iRacing connector`.
    pub name: &'static str,
    /// The simulator's id in the data model, e.g. `iracing`.
    pub sim: &'static str,
    /// The simulator's display name, e.g. `iRacing`.
    pub sim_name: &'static str,
    /// The product key: a UUID version 4, lowercase, generated once and never changed. Every
    /// process names it in the hello metadata and the descriptor carries it; a setup or
    /// `--register` refuses to overwrite a descriptor holding another key.
    pub product: &'static str,
    /// The connector's version (its `env!("CARGO_PKG_VERSION")`).
    pub version: &'static str,
    /// Prefix of the executables' names: `iracing` gives `iracing-recorder.exe`,
    /// `iracing-connector-ui.exe` and `iracing-connector-window.exe`.
    pub exe_prefix: &'static str,
    /// Prefix of the kernel object names: `IRacingConnector` gives `Local\IRacingConnector.UI`.
    pub mutex_prefix: &'static str,
    /// The name of the `HKCU\...\Run` value that starts the control app with Windows.
    pub autostart_value: &'static str,
    /// How to recognise the game.
    pub game: GameSpec,
    /// The default of `recorder_hz` in `ui_config.json`.
    pub recorder_hz: f64,
    /// The features the descriptor declares, a subset of [`FEATURES`].
    pub features: &'static [&'static str],
    /// The embedded English UI catalog, as JSON text.
    pub ui_catalog: &'static str,
    /// The mark drawn on the icons when `icon` is `None`.
    pub glyph: Glyph,
    /// The connector's own artwork. With `Some`, the tray shows it with a badge of the level
    /// and the window shows it alone; with `None`, both draw `glyph`.
    pub icon: Option<ConnectorIcon>,
    /// Environment variables read as the development tag after `STINTMATE_DEV_INSTANCE`
    /// (for a connector that had its own variable before).
    pub dev_env_aliases: &'static [&'static str],
}

fn is_id(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// `^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`.
fn is_uuid_v4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    parts.len() == 5
        && parts.iter().zip(lens).all(|(p, n)| {
            p.len() == n
                && p.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        && parts[2].starts_with('4')
        && matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b')
}

impl ConnectorSpec {
    /// The connector's IPC endpoint id: `connector-<id>`.
    pub fn endpoint(&self) -> String {
        format!("connector-{}", self.id)
    }

    /// The control app's application id: `<exe_prefix>-connector-ui`.
    pub fn ui_app_id(&self) -> String {
        format!("{}-connector-ui", self.exe_prefix)
    }

    /// The UI catalog's id: `<id>-connector`.
    pub fn catalog_id(&self) -> String {
        format!("{}-connector", self.id)
    }

    /// The recorder's file name: `<exe_prefix>-recorder.exe`.
    pub fn recorder_exe(&self) -> String {
        format!("{}-recorder.exe", self.exe_prefix)
    }

    /// The settings window's file name: `<exe_prefix>-connector-window.exe`.
    pub fn window_exe(&self) -> String {
        format!("{}-connector-window.exe", self.exe_prefix)
    }

    /// Check every value the runtime builds names from; all problems are returned.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if !is_id(self.id) {
            problems.push(format!(
                "id {:?} does not match ^[a-z0-9][a-z0-9_-]{{0,31}}$",
                self.id
            ));
        }
        if !is_uuid_v4(self.product) {
            problems.push(format!(
                "product {:?} is not a lowercase UUID version 4",
                self.product
            ));
        }
        if self.exe_prefix.is_empty()
            || !self
                .exe_prefix
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        {
            problems.push(format!(
                "exe_prefix {:?} does not match ^[a-z0-9-]+$",
                self.exe_prefix
            ));
        }
        if self.mutex_prefix.is_empty()
            || !self.mutex_prefix.bytes().all(|c| c.is_ascii_alphanumeric())
        {
            problems.push(format!(
                "mutex_prefix {:?} does not match ^[A-Za-z0-9]+$",
                self.mutex_prefix
            ));
        }
        for f in self.features {
            if !FEATURES.contains(f) {
                problems.push(format!("feature {f:?} is not in the contract vocabulary"));
            }
        }
        if !(HZ_MIN..=HZ_MAX).contains(&self.recorder_hz) {
            problems.push(format!(
                "recorder_hz {} is outside {HZ_MIN}..={HZ_MAX}",
                self.recorder_hz
            ));
        }
        if self.game.exes.is_empty() {
            problems.push("game.exes is empty".into());
        }
        for e in self.game.exes {
            if e.is_empty() || *e != e.to_lowercase() {
                problems.push(format!("game exe {e:?} is not a lowercase image name"));
            }
        }
        if self.game.window_title.is_some() != self.game.window_class.is_some() {
            problems.push(
                "game.window_title and game.window_class are set together or not at all".into(),
            );
        }
        if let Some(t) = self.game.window_title
            && t != t.to_lowercase()
        {
            problems.push(format!("game.window_title {t:?} is not lowercase"));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }
}

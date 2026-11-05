//! The control app's own options, `<data>\ui_config.json` (machine-local).
//!
//! `start_with_app`: which services the app starts when it runs alone, one switch per service.
//! The app keeps it as the user left the services (a hand switch remembers it; Exit and
//! logoff do not); with no setting the Recorder starts. A missing or corrupt file, or a bad
//! value, falls back to the defaults (per key; per service for `start_with_app`).

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// The file name inside the data folder.
pub const FILE_NAME: &str = "ui_config.json";
/// The logs the Logs pane can show.
pub const LOGS: [&str; 2] = ["recorder", "ui"];
/// The recorder rate bounds, in Hz.
pub const HZ_MIN: f64 = 1.0;
/// The recorder rate bounds, in Hz.
pub const HZ_MAX: f64 = 200.0;

/// The options.
#[derive(Debug, Clone, PartialEq)]
pub struct UiConfig {
    /// Start with the window hidden.
    pub start_minimized: bool,
    /// The recorder's rate (`--hz`).
    pub recorder_hz: f64,
    /// The log the Logs pane opens on.
    pub logs_default: String,
    /// Per service: start it with the app.
    pub start_with_app: BTreeMap<String, bool>,
}

impl UiConfig {
    /// The defaults of a connector whose recorder runs at `recorder_hz` unless told otherwise.
    pub fn new(recorder_hz: f64) -> Self {
        Self {
            start_minimized: true,
            recorder_hz,
            logs_default: "recorder".into(),
            start_with_app: BTreeMap::from([("recorder".to_string(), true)]),
        }
    }
}

impl UiConfig {
    /// Whether `service` starts with the app (the default when nothing is stored: the Recorder).
    pub fn starts_with_app(&self, service: &str) -> bool {
        self.start_with_app
            .get(service)
            .copied()
            .unwrap_or(service == "recorder")
    }

    /// The options as the file holds them.
    pub fn to_json(&self) -> Value {
        json!({
            "start_minimized": self.start_minimized,
            "recorder_hz": self.recorder_hz,
            "logs_default": self.logs_default,
            "start_with_app": self.start_with_app,
        })
    }

    /// Checks every value (what [`save`] needs).
    pub fn validate(&self) -> Result<(), String> {
        if !(HZ_MIN..=HZ_MAX).contains(&self.recorder_hz) {
            return Err(format!("invalid recorder_hz: {}", self.recorder_hz));
        }
        if !LOGS.contains(&self.logs_default.as_str()) {
            return Err(format!("invalid logs_default: {}", self.logs_default));
        }
        if let Some(bad) = self
            .start_with_app
            .keys()
            .find(|k| !crate::SERVICES.contains(&k.as_str()))
        {
            return Err(format!("invalid start_with_app service: {bad}"));
        }
        Ok(())
    }
}

/// Reads the options; anything missing or invalid keeps its default (`default_hz` for the
/// recorder rate).
pub fn load(path: &Path, default_hz: f64) -> UiConfig {
    let mut cfg = UiConfig::new(default_hz);
    let Some(Value::Object(data)) = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return cfg;
    };
    if let Some(v) = data.get("start_minimized").and_then(Value::as_bool) {
        cfg.start_minimized = v;
    }
    if let Some(v) = data.get("recorder_hz").and_then(Value::as_f64)
        && (HZ_MIN..=HZ_MAX).contains(&v)
    {
        cfg.recorder_hz = v;
    }
    if let Some(v) = data.get("logs_default").and_then(Value::as_str)
        && LOGS.contains(&v)
    {
        cfg.logs_default = v.into();
    }
    if let Some(Value::Object(m)) = data.get("start_with_app") {
        for (k, v) in m {
            if let (true, Some(b)) = (crate::SERVICES.contains(&k.as_str()), v.as_bool()) {
                cfg.start_with_app.insert(k.clone(), b);
            }
        }
    }
    cfg
}

/// Writes the options atomically. Refuses an invalid value.
pub fn save(cfg: &UiConfig, path: &Path) -> io::Result<()> {
    cfg.validate()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    write_json_atomic(path, &cfg.to_json())
}

/// Writes `value` to `path` through a temp file and a rename (retried briefly: a reader may
/// hold the target open on Windows). Creates the folder.
pub fn write_json_atomic(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    text.push('\n');
    std::fs::write(&tmp, text)?;
    let mut last = None;
    for attempt in 0..10 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(last.unwrap_or_else(|| io::Error::other("rename failed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_without_a_file_or_with_a_corrupt_one() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        assert_eq!(load(&p, 50.0), UiConfig::new(50.0));
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(load(&p, 50.0), UiConfig::new(50.0));
        std::fs::write(&p, "[1]").unwrap();
        assert_eq!(load(&p, 50.0), UiConfig::new(50.0));
        assert!(UiConfig::new(50.0).starts_with_app("recorder"));
        assert!(!UiConfig::new(50.0).starts_with_app("other"));
    }

    #[test]
    fn bad_values_fall_back_one_by_one() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(FILE_NAME);
        std::fs::write(
            &p,
            r#"{"start_minimized": false, "recorder_hz": 500, "logs_default": "ui",
                "start_with_app": {"recorder": false, "gone": true, "x": 3}}"#,
        )
        .unwrap();
        let c = load(&p, 50.0);
        assert!(!c.start_minimized);
        assert_eq!(c.recorder_hz, 50.0, "out of range");
        assert_eq!(c.logs_default, "ui");
        assert!(!c.starts_with_app("recorder"));
        assert_eq!(c.start_with_app.len(), 1, "unknown services are ignored");
    }

    #[test]
    fn save_round_trips_and_refuses_invalid_values() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join(FILE_NAME);
        let mut c = UiConfig {
            recorder_hz: 25.0,
            ..UiConfig::new(50.0)
        };
        c.start_with_app.insert("recorder".into(), false);
        save(&c, &p).unwrap();
        assert_eq!(load(&p, 50.0), c);
        c.recorder_hz = 0.5;
        assert!(save(&c, &p).is_err());
        c.recorder_hz = 50.0;
        c.logs_default = "nope".into();
        assert!(save(&c, &p).is_err());
        assert_eq!(load(&p, 50.0).recorder_hz, 25.0, "the file is untouched");
    }
}

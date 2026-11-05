//! The start-up steps of a recorder process, in order: arguments, spec check, workspace, data
//! folder, logging, single instance, stop event, priority and the two links (not started).
//!
//! Exit codes: bad arguments or spec 2, workspace failure
//! ([`WORKSPACE_ERROR_EXIT`]) 3, another recorder running or no stop event 1, `--version` 0.

use crate::link::{
    ControlLink, PITWALL_ENDPOINT, PitwallLink, SERVICE, StatusPublisher, key_source, peer_info,
};
use std::sync::Arc;
use std::time::Instant;
use stintmate_connector_platform::dev;
use stintmate_connector_platform::location::{self, Location, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use stintmate_connector_platform::logging::{self, LogConfig, LogGuard, local_iso};
use stintmate_connector_platform::proc::lower_process_priority;
use stintmate_connector_platform::single::{Names, SingleInstance, StopEvent};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_connector_platform::workspace::SystemProbes;

/// The command line of a recorder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecorderArgs {
    /// `--workspace DIR`.
    pub workspace: Option<String>,
    /// `--allow-synced`.
    pub allow_synced: bool,
    /// `--hz N`: samples per second (the spec's `recorder_hz` when absent).
    pub hz: Option<f64>,
    /// `--pitwall-pipe NAME`: Pitwall's endpoint (tests).
    pub pitwall_pipe: Option<String>,
    /// `--control-pipe NAME`: the control app's endpoint (tests).
    pub control_pipe: Option<String>,
    /// `--version`: print the name and version, then exit 0.
    pub version: bool,
    /// Start without a link to the control app (set by code, not by a flag).
    pub no_control: bool,
    /// Every other argument, in order: the connector's own flags and subcommands.
    pub rest: Vec<String>,
}

/// Parses the value of `--hz`: a positive, finite number of samples per second.
pub fn parse_hz(value: Option<&str>) -> Result<f64, String> {
    let v = value.ok_or("--hz needs a value")?;
    match v.parse::<f64>() {
        Ok(hz) if hz.is_finite() && hz > 0.0 => Ok(hz),
        _ => Err(format!("--hz needs a positive number, got {v:?}")),
    }
}

impl RecorderArgs {
    /// Splits the arguments (program name excluded). The error is the line to print (exit 2).
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Self, String> {
        let args = WorkspaceArgs::parse(args);
        let mut out = Self {
            workspace: args.workspace,
            allow_synced: args.allow_synced,
            ..Self::default()
        };
        let mut it = args.rest.into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--version" => out.version = true,
                "--hz" => out.hz = Some(parse_hz(it.next().as_deref())?),
                "--pitwall-pipe" => out.pitwall_pipe = it.next(),
                "--control-pipe" => out.control_pipe = it.next(),
                _ => out.rest.push(a),
            }
        }
        Ok(out)
    }
}

/// What [`boot`] hands over; keep it alive for the life of the process.
pub struct Booted {
    /// The resolved workspace and data folder.
    pub location: Location,
    /// Keeps the log file writer flushing (`None` when a logger was already installed).
    pub log_guard: Option<LogGuard>,
    /// The single-instance mutex of this recorder.
    pub instance: SingleInstance,
    /// Set by the control app to ask the recorder to stop.
    pub stop: StopEvent,
    /// The link to Pitwall (not started).
    pub pitwall: Arc<PitwallLink>,
    /// The link to the control app (not started).
    pub control: Option<Arc<ControlLink>>,
    /// Sets `connector.status` and `service.status`.
    pub status: StatusPublisher,
    /// When the process started.
    pub started_at: Instant,
    /// The samples per second: `--hz`, else the spec's default.
    pub hz: f64,
}

/// The pipes of Pitwall and of the control app for development tag `tag` (`None`: the user's
/// real ones).
pub fn pipes(
    spec: &ConnectorSpec,
    tag: Option<&str>,
    args: &RecorderArgs,
) -> Result<(String, String), String> {
    let user = stintmate_ipc::naming::pipe_user().map_err(|e| e.to_string())?;
    let make = |given: &Option<String>, app: &str| match given {
        Some(p) => Ok(p.clone()),
        None => dev::pipe_path_with(app, &user, tag).map_err(|e| e.to_string()),
    };
    Ok((
        make(&args.pitwall_pipe, PITWALL_ENDPOINT)?,
        make(&args.control_pipe, &spec.endpoint())?,
    ))
}

/// Starts a recorder process: `Err(code)` is the exit code of the process (the reason is on
/// stderr or in the log).
pub fn boot(spec: &'static ConnectorSpec, args: &RecorderArgs) -> Result<Booted, i32> {
    boot_with(spec, args, Names::for_spec(spec), dev::tag(spec))
}

/// [`boot`] with the kernel object names and the development tag given (tests).
pub fn boot_with(
    spec: &'static ConnectorSpec,
    args: &RecorderArgs,
    names: Names,
    tag: Option<String>,
) -> Result<Booted, i32> {
    let prog = format!("{}-recorder", spec.exe_prefix);
    if args.version {
        println!("{prog} {}", spec.version);
        return Err(0);
    }
    if let Err(problems) = spec.validate() {
        for p in problems {
            eprintln!("{prog}: invalid connector spec: {p}");
        }
        return Err(2);
    }
    let hz = args.hz.unwrap_or(spec.recorder_hz);
    let location = match location::resolve(
        spec,
        args.workspace.as_deref(),
        args.allow_synced,
        &SystemProbes,
    ) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.line());
            return Err(WORKSPACE_ERROR_EXIT);
        }
    };
    let _ = std::fs::create_dir_all(&location.data_dir);
    let log_guard = logging::init(&LogConfig::new(&location.data_dir, "recorder")).ok();
    logging::install_panic_hook();
    location.log();
    let instance = match SingleInstance::acquire(&names.recorder) {
        Ok(i) if !i.already_running() => i,
        _ => {
            tracing::error!("Another recorder is already running; exiting");
            return Err(1);
        }
    };
    let Ok(stop) = StopEvent::create(&names.recorder_stop) else {
        tracing::error!("Cannot create the stop event");
        return Err(1);
    };
    lower_process_priority();
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        tracing::error!("Cannot start: {what}: {e}");
        1
    };
    let (pitwall_pipe, control_pipe) =
        pipes(spec, tag.as_deref(), args).map_err(|e| fail("pipe names", &e))?;
    let key = key_source(&location.workspace).map_err(|e| fail("key", &e))?;
    let pitwall = Arc::new(
        PitwallLink::new(&pitwall_pipe, peer_info(spec, SERVICE), key)
            .map_err(|e| fail("link to Pitwall", &e))?,
    );
    let control = if args.no_control {
        None
    } else {
        let key = key_source(&location.workspace).map_err(|e| fail("key", &e))?;
        Some(Arc::new(
            ControlLink::new(&control_pipe, peer_info(spec, SERVICE), key)
                .map_err(|e| fail("link to the control app", &e))?,
        ))
    };
    let status = StatusPublisher::new(&pitwall, control.clone(), local_iso(true))
        .map_err(|e| fail("status", &e))?;
    Ok(Booted {
        location,
        log_guard,
        instance,
        stop,
        pitwall,
        control,
        status,
        started_at: Instant::now(),
        hz,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Result<RecorderArgs, String> {
        RecorderArgs::parse(list.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn arguments_are_split_and_the_rest_is_kept() {
        let a = args(&[
            "--workspace",
            "W",
            "--allow-synced",
            "--hz",
            "30",
            "--pitwall-pipe",
            "p",
            "--control-pipe",
            "c",
            "--own-flag",
            "x",
            "announce",
        ])
        .unwrap();
        assert_eq!(a.workspace.as_deref(), Some("W"));
        assert!(a.allow_synced && !a.version);
        assert_eq!(a.hz, Some(30.0));
        assert_eq!(a.pitwall_pipe.as_deref(), Some("p"));
        assert_eq!(a.control_pipe.as_deref(), Some("c"));
        assert_eq!(a.rest, ["--own-flag", "x", "announce"]);
        assert!(args(&["--version"]).unwrap().version);
    }

    #[test]
    fn a_bad_rate_is_an_error() {
        assert!(args(&["--hz"]).is_err());
        assert!(args(&["--hz", "0"]).is_err());
        assert!(args(&["--hz", "fast"]).is_err());
        assert!(args(&["--hz", "inf"]).is_err());
    }
}

//! The command line and the process's life: `--register`, `--unregister`, `--read-registration`
//! (the machine descriptor, no workspace), workspace errors, single instance,
//! the second launch's call, and the headless run until a UI (U1) is attached.

use crate::Hooks;
use crate::api::UiEvent;
use crate::app::{ControlApp, Environment};
use crate::autostart::{Autostart, WinRunKey};
use crate::endpoint::ServeConfig;
use crate::instance::CallFailure;
use crate::services::WinHost;
use crate::texts::Texts;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
use stintmate_connector_platform::location::{self, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use stintmate_connector_platform::logging::{self, LogConfig};
use stintmate_connector_platform::single::{self, Names, SingleInstance};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_connector_platform::workspace::SystemProbes;
use stintmate_ipc::keyfile::{KeyFile, KeySource, default_key_path};

/// What the command line asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// `--tray`: start hidden in the tray.
    pub tray: bool,
    /// `--managed`.
    pub managed: bool,
    /// `--parent-pid <pid>`.
    pub parent_pid: Option<u32>,
    /// `--register`: write the machine descriptor and exit.
    pub register: bool,
    /// `--flavour <name>` (with `--register`): which build this is.
    pub flavour: Option<String>,
    /// `--uninstall <exe>` (with `--register`): the uninstaller that removes this install.
    pub uninstall: Option<String>,
    /// `--unregister`: remove the descriptor (when it names this install) and exit.
    pub unregister: bool,
    /// `--read-registration <out-file>`: write the installed registration for a setup and exit.
    pub read_registration: Option<String>,
    /// `--quit`: ask the running instance to exit.
    pub quit: bool,
    /// `--stintmate-hold`: ask the Stintmate hub to stop and hold this connector.
    pub stintmate_hold: bool,
    /// `--purpose update|uninstall` (with `--stintmate-hold`).
    pub purpose: Option<String>,
    /// `--wait <seconds>` (with `--stintmate-hold`).
    pub wait: Option<u64>,
    /// `--stintmate-release`: ask the Stintmate hub to end the hold and start the connector.
    pub stintmate_release: bool,
    /// `--no-start` (with `--stintmate-release`): end the hold without starting the connector.
    pub no_start: bool,
    /// `--from-shortcut`: what the Start menu shortcut runs: open Stintmate, or say it is needed.
    pub from_shortcut: bool,
    /// `--version`: print the version and exit.
    pub version: bool,
    /// `--workspace`.
    pub workspace: Option<String>,
    /// `--allow-synced`.
    pub allow_synced: bool,
    /// Options (and the bare values after them) this launcher does not know: ignored, so a newer
    /// hub can append options an older connector has never heard of. Logged once at start.
    pub ignored: Vec<String>,
}

/// Parses the arguments (program name excluded); the error is a usage message. Unknown options are
/// not an error: they land in [`Args::ignored`] (the window and the recorder, which only the
/// control app starts, still refuse them).
pub fn parse<I: IntoIterator<Item = String>>(argv: I) -> Result<Args, String> {
    let ws = WorkspaceArgs::parse(argv);
    let mut a = Args {
        workspace: ws.workspace,
        allow_synced: ws.allow_synced,
        ..Args::default()
    };
    let mut it = ws.rest.into_iter().peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tray" => a.tray = true,
            "--managed" => a.managed = true,
            "--register" => a.register = true,
            "--flavour" => {
                let v = it.next().ok_or("--flavour needs a value")?;
                a.flavour = Some(v);
            }
            "--uninstall" => {
                let v = it.next().ok_or("--uninstall needs a value")?;
                a.uninstall = Some(v);
            }
            "--read-registration" => {
                let v = it.next().ok_or("--read-registration needs an out file")?;
                a.read_registration = Some(v);
            }
            "--unregister" => a.unregister = true,
            "--quit" => a.quit = true,
            "--stintmate-hold" => a.stintmate_hold = true,
            "--stintmate-release" => a.stintmate_release = true,
            "--no-start" => a.no_start = true,
            "--purpose" => {
                let v = it.next().ok_or("--purpose needs a value")?;
                if v != "update" && v != "uninstall" {
                    return Err("--purpose must be update or uninstall".into());
                }
                a.purpose = Some(v);
            }
            "--wait" => {
                let v = it.next().ok_or("--wait needs a number of seconds")?;
                a.wait = Some(v.parse().map_err(|_| "--wait needs a number of seconds")?);
            }
            "--from-shortcut" => a.from_shortcut = true,
            "--version" => a.version = true,
            "--parent-pid" => {
                let v = it.next().ok_or("--parent-pid needs a value")?;
                a.parent_pid = Some(v.parse().map_err(|_| "--parent-pid needs a number")?);
            }
            other => {
                // Unknown: kept for the log. A bare token right after an unknown option is its
                // value and is never read as a known option.
                a.ignored.push(other.to_string());
                if other.starts_with('-') && !other.contains('=') {
                    while let Some(v) = it.next_if(|n| !n.starts_with('-')) {
                        a.ignored.push(v);
                    }
                }
            }
        }
    }
    if a.managed && a.parent_pid.is_none_or(|p| p == 0) {
        return Err("--managed needs --parent-pid <pid>".into());
    }
    if a.parent_pid.is_some() && !a.managed {
        return Err("--parent-pid needs --managed".into());
    }
    if (a.flavour.is_some() || a.uninstall.is_some()) && !a.register {
        return Err("--flavour and --uninstall need --register".into());
    }
    let commands = [
        ("--register", a.register),
        ("--unregister", a.unregister),
        ("--read-registration", a.read_registration.is_some()),
        ("--quit", a.quit),
        ("--stintmate-hold", a.stintmate_hold),
        ("--stintmate-release", a.stintmate_release),
        ("--from-shortcut", a.from_shortcut),
        ("--version", a.version),
    ];
    let asked: Vec<&str> = commands
        .iter()
        .filter(|(_, on)| *on)
        .map(|(n, _)| *n)
        .collect();
    if asked.len() > 1 {
        return Err(format!("{} cannot be combined", asked.join(" and ")));
    }
    if let Some(which) = asked.first()
        && (a.managed || a.tray)
    {
        return Err(format!(
            "{which} starts nothing: not with --managed or --tray"
        ));
    }
    if (a.register || a.unregister || a.read_registration.is_some() || a.from_shortcut)
        && (a.workspace.is_some() || a.allow_synced)
    {
        return Err(
            "--register, --unregister, --read-registration and --from-shortcut use no workspace: they act on the machine, not on a workspace"
                .into(),
        );
    }
    if (a.stintmate_hold || a.stintmate_release) && (a.workspace.is_some() || a.allow_synced) {
        return Err(
            "--stintmate-hold and --stintmate-release take no workspace: they use the user's own"
                .into(),
        );
    }
    if (a.purpose.is_some() || a.wait.is_some()) && !a.stintmate_hold {
        return Err("--purpose and --wait need --stintmate-hold".into());
    }
    if a.no_start && !a.stintmate_release {
        return Err("--no-start needs --stintmate-release".into());
    }
    Ok(a)
}

/// The usage message of the control app of `spec`: every option.
pub fn usage(spec: &ConnectorSpec) -> String {
    let exe = spec.ui_app_id();
    let id = spec.id;
    format!(
        "usage: {exe} [--tray] [--managed --parent-pid <pid>] [--workspace <dir>] [--allow-synced]
       {exe} --register [--flavour <name>] [--uninstall <exe>]
       {exe} --unregister
       {exe} --read-registration <out-file>
       {exe} --quit [--workspace <dir>] [--allow-synced]
       {exe} --stintmate-hold [--purpose update|uninstall] [--wait <seconds>]
       {exe} --stintmate-release [--no-start]
       {exe} --from-shortcut
       {exe} --version
  --tray            start hidden in the tray
  --managed         run for a managing app (no tray icon); needs --parent-pid <pid>
  --register        write %LOCALAPPDATA%\\Stintmate\\connectors\\{id}.json (needs no workspace) and exit
  --flavour <name>  with --register: which build this is, for example dev
  --uninstall <exe> with --register: the uninstaller that removes this install
  --unregister      remove that file when it names this install and this connector (logs, outbox and settings stay) and exit
  --read-registration <out-file>
                    write the installed registration (launcher, dir, uninstall, uninstall_args,
                    flavour) as key=value lines for a setup; exit 2 when there is none
  --quit            ask the running app to exit (exit 4 if a managing app owns it)
  --stintmate-hold  for a setup: ask Stintmate to stop this connector and keep it stopped
                    (--purpose update, the default, or uninstall; --wait seconds, default 25)
  --stintmate-release
                    for a setup: ask Stintmate to end the hold and start the connector again
                    (--no-start: only end the hold)
  --from-shortcut   what the Start menu shortcut runs: open Stintmate and exit, or, when Stintmate
                    is not installed, offer its website; never starts a connector
  --version         print the version and exit
  --workspace <dir> the Stintmate workspace; --allow-synced accepts one inside OneDrive
  STINTMATE_CONNECTORS_DIR replaces the connectors folder (tests)
exit status: 0 done, 1 failed, 2 bad arguments (--read-registration: no registration), 3 workspace error, 4 managed (--quit)
  --stintmate-hold: 0 held and stopped, 1 held but something still runs, 5 no Stintmate to ask, 6 Stintmate did not answer, 7 a session is being recorded
  --stintmate-release: 0 released (or nothing held), 5 no Stintmate to ask, 6 Stintmate did not answer"
    )
}

/// Exit status of `--quit` when the running app is managed: it must be stopped by its manager.
pub const QUIT_MANAGED_EXIT: i32 = 4;
/// How long `--quit` waits for the connector to stop.
pub const QUIT_WAIT: Duration = Duration::from_secs(10);

fn say(line: &str) {
    println!("{line}");
}

/// `--version`: the line printed (`<exe name> <version>`).
pub fn version_line(spec: &ConnectorSpec) -> String {
    format!("{} {}", spec.ui_app_id(), spec.version)
}

/// `--unregister`: removes this connector's descriptor, only when its `launcher` names this
/// install (`this_exe`), and nothing else. 0: removed, not there, or another install's (left
/// as it is; one line each), 1: cannot be removed.
pub fn unregister(spec: &ConnectorSpec, this_exe: &std::path::Path) -> i32 {
    use crate::descriptor::Removed;
    match crate::descriptor::remove(spec, this_exe) {
        Ok(Removed::Removed(p)) => {
            say(&format!("unregistered: {}", p.display()));
            0
        }
        Ok(Removed::NotThere) => {
            say("nothing to remove: the connector is not registered");
            0
        }
        Ok(Removed::OtherInstall(p)) => {
            say(&format!(
                "left as it is: {} names another install or is not this connector's",
                p.display()
            ));
            0
        }
        Err(e) => {
            say(&format!("cannot remove the connector descriptor: {e}"));
            1
        }
    }
}

/// `--read-registration`: writes the installed registration to `out` for a setup (UTF-8 with a
/// BOM, `key=value` lines). 0 written, 2 no registration of this connector (nothing written),
/// 1 the out file cannot be written.
pub fn read_registration(spec: &ConnectorSpec, out: &std::path::Path) -> i32 {
    let Some(reg) = crate::descriptor::read_ours(spec) else {
        say(&format!(
            "not registered: no valid {}.json of this connector",
            spec.id
        ));
        return 2;
    };
    match std::fs::write(out, reg.to_file_text()) {
        Ok(()) => {
            say(&format!("registration written: {}", out.display()));
            0
        }
        Err(e) => {
            say(&format!("cannot write {}: {e}", out.display()));
            1
        }
    }
}

/// `--quit`: asks the running control app (found by the mutex `names.ui`) to exit through its
/// endpoint's `control.quit`, and a recorder left running alone through its stop event, then
/// waits up to `wait` for both to be gone. 0: nothing runs anymore (or nothing ran), 1: still
/// running or unreachable, 4: the app is managed (it refuses; its managing app must stop it).
pub fn quit(
    spec: &ConnectorSpec,
    names: &Names,
    pipe: &str,
    key: impl Fn() -> KeySource,
    wait: Duration,
) -> i32 {
    let ui = || single::is_running(&names.ui);
    let recorder = || single::is_running(&names.recorder);
    if !ui() && !recorder() {
        say(&format!("The {} is not running.", spec.name));
        return 0;
    }
    if ui() {
        let reply = crate::instance::call_checked(
            spec,
            pipe,
            key,
            "control.quit",
            &serde_json::json!({}),
            wait.min(Duration::from_secs(3)),
        );
        match reply {
            Ok(_) => {}
            Err(CallFailure::Remote { code, message }) if code == "managed" => {
                say(&format!("not quit: {message}"));
                return QUIT_MANAGED_EXIT;
            }
            Err(CallFailure::Remote { message, .. }) => {
                say(&format!("the running app refused to quit: {message}"));
                return 1;
            }
            Err(CallFailure::Unreachable) => {
                say("the running app does not answer on its endpoint");
                return 1;
            }
        }
    }
    let deadline = Instant::now() + wait;
    let mut recorder_asked = false;
    loop {
        if !ui() && recorder() && !recorder_asked {
            // The app is gone but a recorder started on its own is not: its stop event.
            single::signal_stop(&names.recorder_stop);
            recorder_asked = true;
        }
        if !ui() && !recorder() {
            say(&format!("The {} has stopped.", spec.name));
            return 0;
        }
        if Instant::now() >= deadline {
            say(&format!("The {} is still running.", spec.name));
            return 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `--register`: writes the machine descriptor (needs no workspace) and returns the exit
/// status: 0 written (or already up to date), 1 the file cannot be written or holds another
/// product's registration (left as it is).
pub fn register(spec: &ConnectorSpec, args: &Args, ui_exe: &std::path::Path, mutex: &str) -> i32 {
    let uninstall = args.uninstall.as_deref().map(std::path::Path::new);
    match crate::descriptor::write(spec, ui_exe, mutex, args.flavour.as_deref(), uninstall) {
        Ok(p) => {
            say(&format!("registered: {}", p.display()));
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            say(&format!("not registered: {e}"));
            1
        }
        Err(e) => {
            say(&format!("cannot write the connector descriptor: {e}"));
            1
        }
    }
}

/// The process's entry point: the exit status.
pub fn main(spec: &'static ConnectorSpec, hooks: Hooks, argv: Vec<String>) -> i32 {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(m) => {
            attach_console();
            eprintln!(
                "{}: {m}
{}",
                spec.ui_app_id(),
                usage(spec)
            );
            return 2;
        }
    };
    let names = Names::for_spec(spec);
    let ui_exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from(format!("{}.exe", spec.ui_app_id())));
    if args.version
        || args.register
        || args.unregister
        || args.quit
        || args.stintmate_hold
        || args.stintmate_release
        || args.from_shortcut
        || args.read_registration.is_some()
    {
        attach_console();
    }
    if args.version {
        say(&version_line(spec));
        return 0;
    }
    if args.unregister {
        return unregister(spec, &ui_exe);
    }
    if let Some(out) = &args.read_registration {
        return read_registration(spec, std::path::Path::new(out));
    }
    if args.quit {
        return run_quit(spec, &args, &names);
    }
    if args.from_shortcut {
        return run_from_shortcut(spec);
    }
    if args.stintmate_hold || args.stintmate_release {
        return run_stintmate(spec, &args, &names);
    }
    if args.register {
        return register(spec, &args, &ui_exe, &names.ui);
    }
    let loc = match location::resolve(
        spec,
        args.workspace.as_deref(),
        args.allow_synced,
        &SystemProbes,
    ) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.line());
            #[cfg(windows)]
            if !args.managed {
                let t = Texts::english(spec);
                let body = format!(
                    "{}

{}

{}",
                    t.s("startup.error.title"),
                    e.line(),
                    t.s("startup.error.hint")
                );
                crate::dialog::error(&t.s("app.name"), &body);
            }
            return WORKSPACE_ERROR_EXIT;
        }
    };
    let _ = std::fs::create_dir_all(&loc.data_dir);
    let _guard = logging::init(&LogConfig::new(&loc.data_dir, "ui"));
    logging::install_panic_hook();
    if !args.ignored.is_empty() {
        tracing::warn!("Ignored unknown argument(s): {}", args.ignored.join(" "));
    }
    loc.log();
    let key_path = default_key_path(Some(&loc.workspace));
    let key =
        || -> KeySource { KeySource::File(KeyFile::open(key_path.clone().unwrap_or_default())) };

    // Single instance: a second launch asks the running one to show itself (or to be managed).
    let instance = SingleInstance::acquire(&names.ui);
    if !matches!(&instance, Ok(i) if !i.already_running()) {
        let pipe = stintmate_connector_platform::dev::pipe_path(spec, &spec.endpoint())
            .unwrap_or_default();
        let (method, params) = match args.parent_pid.filter(|_| args.managed) {
            Some(pid) => ("control.managed", serde_json::json!({"parent_pid": pid})),
            None => ("control.show", serde_json::json!({})),
        };
        let reply =
            crate::instance::call(spec, &pipe, key, method, &params, Duration::from_secs(3));
        tracing::info!(
            "The {} app is already running; {method} (reply {reply:?})",
            spec.name
        );
        return 0;
    }

    let recorder_exe = ui_exe.with_file_name(spec.recorder_exe());
    let app = ControlApp::new(Environment {
        spec,
        resume: hooks.resume,
        workspace: loc.workspace.clone(),
        data_dir: loc.data_dir.clone(),
        data_source: loc.describe(),
        workspace_args: loc.child_args(),
        ui_exe: ui_exe.clone(),
        recorder_exe,
        recorder_mutex: names.recorder.clone(),
        recorder_stop: names.recorder_stop.clone(),
        recorder_extra_args: Vec::new(),
        stop_timeout: crate::services::STOP_TIMEOUT,
        host: Box::new(WinHost::new(&spec.game)),
        autostart: Autostart::new(
            spec.autostart_value,
            Box::new(WinRunKey::default()),
            &ui_exe,
        ),
        texts: Texts::load(spec, Some(&loc.workspace)),
        parent_watch: Box::new(crate::managed::ParentWatch::new),
        tray_available: tray_available(),
        time: None,
    });
    let events = app.take_events();
    let serve = ServeConfig {
        pipe: None,
        registry: None,
        key: key(),
    };
    if let Err(e) = app.serve(serve) {
        tracing::error!(
            "Cannot serve the connector's endpoint ({e}); a managing app and the services cannot reach this app"
        );
    }
    tracing::info!(
        "{} app started (pid {}, managed={:?}, started by {})",
        spec.name,
        std::process::id(),
        args.parent_pid.filter(|_| args.managed),
        stintmate_connector_platform::procinfo::describe_process(
            stintmate_connector_platform::procinfo::process_chain(std::process::id(), 2)
                .get(1)
                .map(|c| c.0),
            4
        )
    );
    app.start(args.parent_pid.filter(|_| args.managed));
    let ticker = app.spawn_ticker();
    if let Some(events) = events {
        run_ui(spec, &app, events, &ui_exe, &loc, &args);
    }
    let _ = ticker.join();
    tracing::info!("{} app exited", spec.name);
    0
}

/// Makes the output of the command-line options reach the console that started this
/// GUI-subsystem exe (when its output is not redirected).
fn attach_console() {
    #[cfg(windows)]
    stintmate_connector_platform::console::attach_parent_output();
}

/// `--from-shortcut` on this machine: the texts come from the user's workspace language when
/// the pointer names a workspace, English otherwise.
#[cfg(windows)]
fn run_from_shortcut(spec: &'static ConnectorSpec) -> i32 {
    let workspace = location::resolve(spec, None, false, &SystemProbes)
        .ok()
        .map(|l| l.workspace);
    crate::shortcut::run(
        &crate::shortcut::StintmateProbe { spec },
        &Texts::load(spec, workspace.as_deref()),
    )
}

/// `--from-shortcut` needs Windows.
#[cfg(not(windows))]
fn run_from_shortcut(_spec: &'static ConnectorSpec) -> i32 {
    1
}

/// `--quit` against the real names, the endpoint and the workspace's key.
fn run_quit(spec: &'static ConnectorSpec, args: &Args, names: &Names) -> i32 {
    let workspace = location::resolve(
        spec,
        args.workspace.as_deref(),
        args.allow_synced,
        &SystemProbes,
    )
    .ok()
    .map(|l| l.workspace);
    let key_path = default_key_path(workspace.as_deref());
    let pipe =
        stintmate_connector_platform::dev::pipe_path(spec, &spec.endpoint()).unwrap_or_default();
    quit(
        spec,
        names,
        &pipe,
        || KeySource::File(KeyFile::open(key_path.clone().unwrap_or_default())),
        QUIT_WAIT,
    )
}

/// `--stintmate-hold` / `--stintmate-release` against the user's own hub: the workspace is
/// resolved like `--quit` (the pointer), the key is the workspace's. Exit 5 when there is no
/// pointer, no workspace or no key: there is no Stintmate to ask.
fn run_stintmate(spec: &'static ConnectorSpec, args: &Args, names: &Names) -> i32 {
    let workspace = match location::resolve(spec, None, false, &SystemProbes) {
        Ok(l) => l.workspace,
        Err(e) => {
            say(&format!("no Stintmate to ask: {}", e.line()));
            return crate::hold::EXIT_NO_HUB;
        }
    };
    let key_path = match default_key_path(Some(&workspace)) {
        Ok(p) if p.is_file() => p,
        _ => {
            say("no Stintmate to ask: the workspace has no key");
            return crate::hold::EXIT_NO_HUB;
        }
    };
    let key = || KeySource::File(KeyFile::open(key_path.clone()));
    let target = crate::hold::HubTarget::production(spec);
    if args.stintmate_hold {
        let wait = Duration::from_secs(args.wait.unwrap_or(crate::hold::DEFAULT_WAIT_S));
        crate::hold::hold(
            names,
            &target,
            key,
            args.purpose.as_deref().unwrap_or("update"),
            wait,
        )
    } else {
        crate::hold::release(&target, key, !args.no_start)
    }
}

#[cfg(windows)]
fn tray_available() -> bool {
    crate::tray::tray_available()
}

#[cfg(not(windows))]
fn tray_available() -> bool {
    false
}

/// The UI of the process: the tray on this thread until the control app quits; the window is
/// another process, started on demand.
#[cfg(windows)]
fn run_ui(
    spec: &'static ConnectorSpec,
    app: &std::sync::Arc<ControlApp>,
    events: Receiver<UiEvent>,
    ui_exe: &std::path::Path,
    loc: &location::Location,
    args: &Args,
) {
    use crate::window_host::WindowHost;
    let host = std::sync::Arc::new(WindowHost::new(
        WindowHost::sibling_exe(spec, ui_exe),
        loc.child_args(),
    ));
    let show_at_start = !args.tray && !app.config().start_minimized && !args.managed;
    crate::tray::run(crate::tray::TrayRun {
        app: app.clone(),
        events,
        texts: Texts::load(spec, Some(&loc.workspace)),
        host,
        show_at_start,
    });
}

/// Without a tray (not Windows) the events are only drained.
#[cfg(not(windows))]
fn run_ui(
    _spec: &'static ConnectorSpec,
    _app: &std::sync::Arc<ControlApp>,
    events: Receiver<UiEvent>,
    _ui_exe: &std::path::Path,
    _loc: &location::Location,
    _args: &Args,
) {
    for ev in events {
        if ev == UiEvent::Quit {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testspec::SPEC as TEST_SPEC;

    fn p(a: &[&str]) -> Result<Args, String> {
        parse(a.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn managed_needs_a_parent_and_the_other_way_round() {
        assert!(p(&["--managed"]).is_err());
        assert!(p(&["--parent-pid", "5"]).is_err());
        assert!(p(&["--managed", "--parent-pid", "0"]).is_err());
        let a = p(&[
            "--managed",
            "--parent-pid",
            "5",
            "--workspace",
            "C:\\w",
            "--allow-synced",
        ])
        .unwrap();
        assert!(a.managed && a.allow_synced);
        assert_eq!(
            (a.parent_pid, a.workspace.as_deref()),
            (Some(5), Some("C:\\w"))
        );
        assert!(p(&["--register", "--managed", "--parent-pid", "5"]).is_err());
        assert!(p(&["--register"]).unwrap().register);
        assert_eq!(p(&["--register", "--check"]).unwrap().ignored, ["--check"]);
        let r = p(&["--register", "--flavour", "dev", "--uninstall", "C:\\u.exe"]).unwrap();
        assert_eq!(
            (r.flavour.as_deref(), r.uninstall.as_deref()),
            (Some("dev"), Some("C:\\u.exe"))
        );
        assert!(p(&["--flavour", "dev"]).is_err());
        assert!(p(&["--unregister", "--uninstall", "x"]).is_err());
        assert!(p(&["--register", "--flavour"]).is_err());
        assert_eq!(
            p(&["--read-registration", "o.txt"])
                .unwrap()
                .read_registration
                .as_deref(),
            Some("o.txt")
        );
        assert!(p(&["--read-registration"]).is_err());
        assert!(p(&["--tray"]).unwrap().tray);
        assert_eq!(p(&["--bogus"]).unwrap().ignored, ["--bogus"]);
    }

    #[test]
    fn unknown_options_are_ignored() {
        let a = p(&[
            "--tray",
            "--frobnicate",
            "--new-hub-flag=1",
            "--workspace",
            "X",
        ])
        .unwrap();
        assert!(a.tray);
        assert_eq!(a.workspace.as_deref(), Some("X"));
        assert_eq!(a.ignored, ["--frobnicate", "--new-hub-flag=1"]);
    }

    #[test]
    fn the_value_of_an_unknown_option_is_not_a_known_option() {
        let a = p(&["--frobnicate", "quit", "--tray"]).unwrap();
        assert!(a.tray && !a.quit);
        assert_eq!(a.ignored, ["--frobnicate", "quit"]);
        let a = p(&["--mode", "x", "stray", "--version"]).unwrap();
        assert!(a.version);
        assert_eq!(a.ignored, ["--mode", "x", "stray"]);
    }

    #[test]
    fn known_options_keep_their_errors_beside_unknown_ones() {
        assert!(p(&["--frobnicate", "--parent-pid", "x"]).is_err());
        assert!(p(&["--frobnicate", "--stintmate-hold", "--wait"]).is_err());
        assert!(p(&["--frobnicate", "--managed"]).is_err());
    }

    #[test]
    fn the_command_options_are_alternatives_and_start_nothing() {
        assert!(p(&["--unregister"]).unwrap().unregister);
        assert!(p(&["--quit"]).unwrap().quit);
        assert!(p(&["--version"]).unwrap().version);
        assert!(p(&["--unregister", "--workspace", "C:/w"]).is_err());
        assert!(p(&["--register", "--allow-synced"]).is_err());
        assert!(p(&["--quit", "--workspace", "C:/w"]).is_ok());
        assert!(p(&["--register", "--quit"]).is_err());
        assert!(p(&["--unregister", "--managed", "--parent-pid", "5"]).is_err());
        assert!(p(&["--quit", "--tray"]).is_err());
        assert!(p(&["--stintmate-hold", "--quit"]).is_err());
        assert!(p(&["--stintmate-hold", "--stintmate-release"]).is_err());
        assert!(p(&["--stintmate-release", "--tray"]).is_err());
        assert_eq!(
            version_line(&TEST_SPEC),
            format!("example-connector-ui {}", TEST_SPEC.version)
        );
    }

    #[test]
    fn the_hub_options_parse_and_check_their_companions() {
        let h = p(&["--stintmate-hold"]).unwrap();
        assert!(h.stintmate_hold && h.purpose.is_none() && h.wait.is_none());
        let h = p(&["--stintmate-hold", "--purpose", "uninstall", "--wait", "7"]).unwrap();
        assert_eq!((h.purpose.as_deref(), h.wait), (Some("uninstall"), Some(7)));
        assert!(p(&["--stintmate-hold", "--purpose", "other"]).is_err());
        assert!(p(&["--stintmate-hold", "--purpose"]).is_err());
        assert!(p(&["--stintmate-hold", "--wait", "x"]).is_err());
        assert!(p(&["--purpose", "update"]).is_err());
        assert!(p(&["--wait", "5"]).is_err());
        let r = p(&["--stintmate-release", "--no-start"]).unwrap();
        assert!(r.stintmate_release && r.no_start);
        assert!(p(&["--no-start"]).is_err());
        assert!(p(&["--stintmate-hold", "--no-start"]).is_err());
        assert!(p(&["--stintmate-release", "--wait", "5"]).is_err());
        assert!(p(&["--stintmate-hold", "--workspace", "C:/w"]).is_err());
        assert!(p(&["--stintmate-release", "--allow-synced"]).is_err());
    }

    #[test]
    fn usage_and_version_name_the_spec() {
        let u = usage(&TEST_SPEC);
        assert!(u.starts_with("usage: example-connector-ui [--tray]"));
        assert!(u.contains("connectors\\example.json"));
        assert!(!u.contains("{exe}") && !u.contains("{id}"));
    }
}

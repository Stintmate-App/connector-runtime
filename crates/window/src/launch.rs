//! The window process's `main`, shared by every connector.

use crate::gui::{Page, Window};
use crate::link::Link;
use eframe::egui;
use stintmate_connector_control::texts::Texts;
use stintmate_connector_platform::location::{self, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use stintmate_connector_platform::logging::{self, LogConfig};
use stintmate_connector_platform::single::{Names, SingleInstance};
use stintmate_connector_platform::spec::ConnectorSpec;
use stintmate_connector_platform::workspace::SystemProbes;
use stintmate_ipc::keyfile::{KeyFile, KeySource, default_key_path};

/// `--page logs`, and the test overrides `--pipe <path>`, `--key-file <path>` and
/// `--mutex <name>` (a test never touches a running connector's endpoint or mutex).
#[derive(Default)]
struct Args {
    page: Option<String>,
    pipe: Option<String>,
    key_file: Option<String>,
    mutex: Option<String>,
}

fn parse(rest: Vec<String>) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--page" => a.page = Some(value("--page")?),
            "--pipe" => a.pipe = Some(value("--pipe")?),
            "--key-file" => a.key_file = Some(value("--key-file")?),
            "--mutex" => a.mutex = Some(value("--mutex")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

/// Runs the window of `spec` with the command line `argv` (the program name excluded) and
/// returns the process exit code: 0 after a normal close (or when a window is already open),
/// 1 when it cannot connect or draw, 2 for a bad command line or spec, and the workspace error
/// code when the workspace cannot be resolved.
///
/// The window is started only by the control app; `--version` is the executable's own
/// business and is not handled here.
pub fn run(spec: &'static ConnectorSpec, argv: Vec<String>) -> i32 {
    let exe = spec.window_exe();
    if let Err(problems) = spec.validate() {
        for p in problems {
            eprintln!("{exe}: invalid connector spec: {p}");
        }
        return 2;
    }
    let ws = WorkspaceArgs::parse(argv);
    let args = match parse(ws.rest.clone()) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("{exe}: {m}");
            return 2;
        }
    };
    let loc = match location::resolve(
        spec,
        ws.workspace.as_deref(),
        ws.allow_synced,
        &SystemProbes,
    ) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.line());
            return WORKSPACE_ERROR_EXIT;
        }
    };
    let _ = std::fs::create_dir_all(&loc.data_dir);
    let _guard = logging::init(&LogConfig::new(&loc.data_dir, "window"));
    logging::install_panic_hook();
    let mutex = args
        .mutex
        .clone()
        .unwrap_or_else(|| Names::for_spec(spec).window);
    let instance = SingleInstance::acquire(&mutex);
    if !matches!(&instance, Ok(i) if !i.already_running()) {
        // The control app tells the running window to come forward (`window_request`).
        tracing::info!("The window is already open");
        return 0;
    }
    let pipe = args.pipe.clone().unwrap_or_else(|| {
        stintmate_connector_platform::dev::pipe_path(spec, &spec.endpoint()).unwrap_or_default()
    });
    let key_path = args
        .key_file
        .clone()
        .map(std::path::PathBuf::from)
        .or_else(|| default_key_path(Some(&loc.workspace)).ok())
        .unwrap_or_default();
    let key = KeySource::File(KeyFile::open(key_path));
    let ctx_cell: std::sync::Arc<std::sync::OnceLock<egui::Context>> = Default::default();
    let wake_cell = ctx_cell.clone();
    let link = match Link::connect(spec, &pipe, key, move || {
        if let Some(ctx) = wake_cell.get() {
            ctx.request_repaint();
        }
    }) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Cannot connect to the control app: {e}");
            return 1;
        }
    };
    let texts = Texts::load(spec, Some(&loc.workspace));
    let page = if args.page.as_deref() == Some("logs") {
        Page::Logs
    } else {
        Page::Services
    };
    let title = texts.s("app.name");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 460.0])
            .with_min_inner_size([560.0, 380.0])
            .with_title(&title)
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: stintmate_connector_resources::render_rgba(spec.glyph, "ok", 64),
                width: 64,
                height: 64,
            })),
        ..Default::default()
    };
    let result = eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            crate::theme::apply(&cc.egui_ctx);
            let _ = ctx_cell.set(cc.egui_ctx.clone());
            Ok(Box::new(Window::new(link, texts, page)))
        }),
    );
    if let Err(e) = result {
        tracing::error!("The window failed: {e}");
        return 1;
    }
    0
}

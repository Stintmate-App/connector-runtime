# Writing a connector on the runtime

This page goes through what a connector supplies. [architecture.md](architecture.md) explains
what the runtime does with it. The contract a connector must keep is the
[connector kit](https://github.com/Stintmate-App/connector-kit)'s; start with its guide.

The smallest working example is in this repository: a spec of made-up values,
`stintmate_connector_testing::EXAMPLE_SPEC`, and the executables
`crates/control/examples/example-connector-ui.rs` and
`crates/window/examples/example_window.rs`, which are the control app's and the window's whole
`main` for the example connector.

## 1. Dependencies

See "Using it" in the [README](../README.md): the crates as git dependencies with
`tag = "v0.4.0"`, and `stintmate-ipc` at the tag named in this repository's `Cargo.toml`
(v0.3.3 for release 0.4.0). The recorder needs `platform`, `wire` and `link`; the control app
needs `platform` and `control`; the window needs `window`; `resources` goes in
`[build-dependencies]` and `testing` in `[dev-dependencies]`.

## 2. The `ConnectorSpec`

Declare one `static` and give a reference to every entry point. `ConnectorSpec::validate()`
returns every problem at once; `control::run`, `window::run` and `link::boot::boot` call it first
and exit with 2 when it fails (a connector's tests should call it too).

```rust
use stintmate_connector_platform::spec::{ConnectorSpec, GameSpec};
use stintmate_connector_resources::Glyph;

pub static SPEC: ConnectorSpec = ConnectorSpec {
    id: "example",
    name: "Example connector",
    sim: "example",
    sim_name: "Example Simulator",
    product: "6f1c2a84-3b7e-4d59-9a0c-5e8d7b2f1a43",
    version: env!("CARGO_PKG_VERSION"),
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
    features: &["live_frame", "lap_status"],
    ui_catalog: include_str!("../locales/en.json"),
    glyph: Glyph::I,
    icon: None,
    dev_env_aliases: &[],
};
```

| Field | What it is | Rule checked by `validate` |
|---|---|---|
| `id` | the connector's id: its data folder `<workspace>\connectors\<id>`, the descriptor file `<id>.json`, the hello's `connector_id`, the idempotency-key prefix, the endpoint `connector-<id>` and the catalog id `<id>-connector` | `^[a-z0-9][a-z0-9_-]{0,31}$` |
| `name` | the display name, used in the hello, logs and `--quit` messages | none |
| `sim`, `sim_name` | the simulator's id in the data model and its display name | none |
| `product` | the product key: a UUID version 4, lowercase, generated once and never changed. Every process names it in the hello and the descriptor carries it; `--register` refuses to overwrite a descriptor with another key | lowercase UUID v4 |
| `version` | the connector's version, printed by `--version` and written to the descriptor | none |
| `exe_prefix` | prefix of the three executables: `<exe_prefix>-recorder.exe`, `<exe_prefix>-connector-ui.exe`, `<exe_prefix>-connector-window.exe` | `^[a-z0-9-]+$` |
| `mutex_prefix` | prefix of the kernel object names: `Local\<mutex_prefix>.UI`, `.Window`, `.Recorder`, `.Recorder.Stop` | `^[A-Za-z0-9]+$` |
| `autostart_value` | the name of the `HKCU\...\Run` value that starts the control app with Windows | none |
| `game` | how to recognise the game, below | see below |
| `recorder_hz` | the default of `recorder_hz` in `ui_config.json`, and the recorder's rate when `--hz` is absent | within `1.0..=200.0` |
| `features` | the features the descriptor declares, a subset of `spec::FEATURES` (the connector kit's vocabulary, 21 names); the descriptor lists the control app's own five first | every name in `FEATURES` |
| `ui_catalog` | the embedded English UI catalog, as JSON text (section 5) | none; checked by `check_ui_catalog` |
| `glyph` | the mark drawn on the icons when `icon` is `None`: `Glyph::A`, `Glyph::I` or your own strokes | none |
| `icon` | the connector's own artwork, `Some(ConnectorIcon { png_32, png_64 })` with two 8-bit RGBA PNG files (32 x 32 and 64 x 64) embedded with `include_bytes!`, or `None` to draw the glyph. The tray shows the artwork with a badge of the level in its lower right corner (grey off, green ok, amber warn, red error); the window shows the artwork alone. A file that does not decode is logged and the glyph is drawn instead; call `icon.rgba(32)` and `icon.rgba(64)` in a test of yours to catch it at build time | none |
| `dev_env_aliases` | environment variables also read as the development tag, after `STINTMATE_DEV_INSTANCE`, for a connector that had its own variable before | none |

`GameSpec`: `exes` are the game's executable image names, lowercase and not empty.
`window_title` (lowercase, matched exactly after trimming) and `window_class` must both be set or
both be `None`; the game window is found by process image first, and by title and class only when both are
set; the window with the largest client area wins (`platform::window::find_game_window`).
`cheap_hint` is an optional `fn() -> bool`, a cheap "the game may run" signal asked between full
process snapshots. With a window title and class or a hint, the full snapshot runs every 60 s
(`game::FULL_CHECK`) and the cheap signal in between; with neither, it runs every 5 s
(`game::CHECK_INTERVAL`).

## 3. The three executables

The control app and the window are Windows GUI programs. Their `main` is one call, as in the
[README](../README.md#using-it); the examples in this repository use
`#![windows_subsystem = "windows"]`.

```rust
// the control app's main.rs
use stintmate_connector_control::{Hooks, run};
fn main() {
    let argv = std::env::args().skip(1).collect();
    std::process::exit(run(&SPEC, Hooks::default(), argv));
}
```

`Hooks` holds the one thing that reads the game in the control app, `resume: Arc<dyn
ResumeProbe>` (section 6). `Hooks::default()` uses `NoResume`.

The window's `main` calls `stintmate_connector_window::run(&SPEC, argv)` and nothing else.
`--version` is the executable's own business and the window crate does not handle it.

The recorder's `main` parses with `RecorderArgs::parse` (the connector's own flags and
subcommands arrive in `rest`), calls `boot(&SPEC, &args)`, starts the two links, and then runs the
connector's own loop: read the simulator at `booted.hz`, feed a `WriteSink`, fill a
`StatusInput` and hand it to a `StatusWorker`, and leave when `booted.stop.is_set()`. On the way
out, close the sink (`WriteSink::close(drain)`; `sink::CLOSE_DRAIN` is 5 s) and the links
(`PitwallLink::close`, `ControlLink::close`). A recorder is stopped by the control app within
10 s before it is terminated, so keep the drain short.

## 4. `build.rs`

The runtime draws the icons and writes the Windows resource script; the connector's `build.rs`
writes the files and compiles them with the `embed-resource` crate. Make one `.ico` per
executable (the tray draws its level icons at run time, from the artwork or the glyph).

A connector with its own artwork keeps a checked-in `.ico` (with at least the 16, 32, 48 and 256 px
images) beside its PNG files and passes that path to `version_rc` instead of writing one with
`ico_bytes`; no helper is needed. In the spec, `icon: Some(ConnectorIcon { png_32:
include_bytes!("../assets/icon_32.png"), png_64: include_bytes!("../assets/icon_64.png") })`
with the 32 x 32 and 64 x 64 PNG files (8-bit RGBA). The `artwork` feature of
`stintmate-connector-resources` (decoding, on in the control and window crates) is only needed to
call `rgba` or `badged_rgba` yourself, for example in a test; a `build.rs` that only needs
`version_rc` leaves it off and keeps the crate on the standard library.

```rust
use stintmate_connector_resources::{FileInfo, Glyph, ico_bytes, version_rc};

fn main() {
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("recorder.ico");
    std::fs::write(&ico, ico_bytes(Glyph::I, "ok", &[16, 32, 48, 64, 128])).unwrap();
    let rc = version_rc(
        &FileInfo {
            description: "Example connector recorder",
            internal_name: "example-recorder",
            original_filename: "example-recorder.exe",
            product_name: "Stintmate Example connector",
            version: env!("CARGO_PKG_VERSION"),
        },
        &ico,
    );
    let rc_path = out.join("recorder.rc");
    std::fs::write(&rc_path, rc).unwrap();
    let _ = embed_resource::compile(&rc_path, embed_resource::NONE);
}
```

`ico_bytes(glyph, level, sizes)` takes sizes of at most 255 pixels. `level` is `off`, `ok`,
`warn` or `error`. `version_rc` takes the numeric `FILEVERSION` from the first three digit
groups of `version` and escapes quotes and backslashes in the strings.

## 5. The UI catalog

Ship an English catalog file (catalog id `<id>-connector`, the format of the connector kit's
`catalogs.md`) and embed it as `ui_catalog`. It must hold every key in
`control::texts::REQUIRED_KEYS` (the keys the control app and the window use, each with its
placeholders) and no text may use other placeholders. A connector's test calls the checker, which
returns the problems found, empty when the catalog is good:

```rust
#[test]
fn the_ui_catalog_is_complete() {
    let problems = stintmate_connector_control::texts::check_ui_catalog(
        SPEC.ui_catalog,
        &SPEC.catalog_id(),
    );
    assert!(problems.is_empty(), "{problems:#?}");
}
```

`crates/control/tests/fixtures/en.example.json` is a complete catalog to copy from. The
announcement catalog (what the engineer may say) is a separate file; parse it once with
`AnnounceCatalog::parse(json, expected_id)` into a `static` and give it to the `Announcer`.

## 6. `ResumeProbe` and `ResumeClock`

Two small traits connect a stopped recorder's session to the game.

- `control::ResumeProbe::look(&self, stored_clock: &serde_json::Value, game_running: bool) ->
  ResumeLook` runs in the control app, about once a second, only while `session-resume.json`
  exists and no recorder runs. Answer `Keep` while the game is still in the stored session (or
  in a menu between its parts), `Drop` when it left it, `Gone` when the game is not running and
  `NoReading` when you cannot tell now. The file is deleted on `Gone` and `Drop`.
  `stored_clock` is whatever your `ResumeClock::to_json` wrote.
- `link::sink::ResumeClock` is the type the sink keeps as the game's session clock:
  `to_json`, `from_json` (`None` when the value is not one) and `continues_as(&self, now:
  &Self) -> bool`, which says whether a session stored with this clock can be the one that now
  reads `now`.

## 7. Writing with `WriteSink`

```rust
use stintmate_connector_link::sink::{SessionOpen, WriteSink};

let mut sink: WriteSink<MyClock> =
    WriteSink::new(&outbox_dir, booted.pitwall.clone(), None, SPEC.id)?;
let session = sink.start_session(SessionOpen {
    identity: format!("{car}|{track}|{layout}|{session_type}"),
    catalog: None,
    start: session_start_message,
});
sink.lap(session, Box::new(move || build_lap_message()));
sink.end_session(session, &ended_at);
```

- `outbox_dir` is a folder the sink owns; `session-resume.json` is written next to it. Use a
  folder inside the connector's data folder (`booted.location.data_dir`).
- `ik_prefix` must be the connector's id (`SPEC.id`), as the idempotency keys of the contract
  start with it.
- `start_session` returns the session's local number for the other calls: `lap`, `setup`,
  `pit_exit`, `participants`, `sim_data`, `end_session`, and `catalog` for cars first seen after the
  session opened. Builders (`LapBuild`, `SetupBuild`) run on the sink's writer thread.
- **Simulator data.** Values the game gives at a moment (a stop, a reset, a garage visit, a
  penalty notice, a damage report) that Pitwall has no typed message for go in `sim_data`:
  `sink.sim_data(session, "kind_name", &taken_at, data)` with `data` a `serde_json::Map` of your
  own design, and `sink.sim_data_kinds(kinds)` with a `SimDataKind` per kind (what a record is, an
  optional `topic`, and the meaning and unit of every field path; a `*` segment stands for any key
  or array index). Describe your kinds once at start: the sink sends the description again before
  every session start under a key made of its content, so Pitwall stores it once. A kind's fields
  never change meaning: a changed meaning is a new kind name. Per-sample data is not for
  `sim_data`: use a lap's channels. Both calls check their input with
  `stintmate_connector_wire::sim_data` (`check_data`, `check_kinds`, the limits as constants: 16,384
  bytes of compact JSON, 6 levels, 512 keys, 256 items per array, 500 characters per string, 5,000
  records per session) and log a refusal at error level instead of queueing a write Pitwall would
  refuse; call `check_data` in your own tests to see the reason. Send every lap that ended before
  the data first, and a stop's data before its `pit_exit`. A Pitwall that predates simulator data
  answers `unknown_method`; the outbox drops those writes.
- **Better positions for laps already sent.** A connector that learns exact positions after the
  fact (the game writes them to a file when the session ends, say) queues each lap with
  `sink.lap_with_address(session, build, Box::new(|key, seq| remember(key, seq)))`, keeps the
  `(key, seq)` pairs, and later sends `sink.lap_positions(&key, seq, Box::new(|| LapPositions {
  channels }))` with `x` and `z` (and optionally `y`), each as long as the lap's channels. It works
  after the session ended. Declare the feature `lap_positions`. A Pitwall that predates the message
  answers `unknown_method`; the outbox drops the write.
- The messages are the structs of `stintmate_connector_wire::messages`. A lap's channels are
  `LapChannels`, serialized as binary `f32` arrays.
- `set_game_token`, `keep_session_open` and `note_clock` feed the resume logic; `stats`,
  `outbox_summary` and the like feed the status.

## 8. The tests to copy

`stintmate-connector-testing` is for `[dev-dependencies]`. `EXAMPLE_SPEC` is a valid spec with
made-up values; `FakePitwall::start(&pipe)` answers `ok` to the write methods and records every
call (`calls`, `count`, `wait_for`); `TempWorkspace` makes a temporary workspace with a test key;
`unique_user()` and `pitwall_pipe(&user)` give pipes that tests running in parallel do not share;
`example_catalog()` is a parsed example announcement catalog.

Copy these from the runtime's own tests, replacing the example spec with yours:

| Test | Where | What it checks |
|---|---|---|
| spec validity | `crates/platform/tests/spec_validation.rs` | `SPEC.validate()` passes and the derived names are what you expect |
| UI catalog | `crates/control/src/texts.rs` (`example_catalog_has_every_required_key`) | your `en.json` against `REQUIRED_KEYS` |
| command line | `crates/control/tests/cli.rs` | `--version`, `--register` and the other options against your built control app (the runtime runs them against the example control app) |
| endpoint | `crates/control/tests/endpoint.rs` | the control interface, including the `stintmate-ipc-check server` conformance run |
| setup hold | `crates/control/tests/hold.rs` | `--stintmate-hold` and `--stintmate-release` against a fake hub endpoint |
| recorder start | `crates/link/tests/boot.rs`, `hello.rs` | a second recorder is refused, the hello carries the spec's values |
| outbox | `crates/link/tests/outbox.rs` | restart, rejected writes and retries over a fake Pitwall |
| window | `crates/window/tests/lifecycle.rs`, `render.rs`, `version.rs` | one window at most, offscreen renders of every page (the lifecycle test is skipped when `STINTMATE_NO_DESKTOP` is set) |

The connector kit also ships a checker, `connector-check`, which tests a connector from outside;
run it as the kit's documentation says.

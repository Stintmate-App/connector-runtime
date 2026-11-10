# Architecture

A connector built on the runtime is three executables of one install. Their names come from the
connector's `exe_prefix` (see [embedding](embedding.md)). The runtime's crates are the code
those executables share.

| Process | Executable | Crate that runs it | Lifetime |
|---|---|---|---|
| control app | `<exe_prefix>-connector-ui.exe` | `stintmate-connector-control` (`run`) | long-lived: the tray icon, the supervision of the recorder, the connector's own endpoint |
| recorder | `<exe_prefix>-recorder.exe` | `stintmate-connector-link` (`boot`) and the connector's own loop | long-lived: reads the simulator, writes to Pitwall; started and stopped by the control app |
| window | `<exe_prefix>-connector-window.exe` | `stintmate-connector-window` (`run`) | short-lived: started by the control app when the user opens it, gone when it closes |

Names are made from the spec: `ConnectorSpec::ui_app_id()` is `<exe_prefix>-connector-ui`,
`recorder_exe()` is `<exe_prefix>-recorder.exe`, `window_exe()` is
`<exe_prefix>-connector-window.exe`, `endpoint()` is `connector-<id>` and `catalog_id()` is
`<id>-connector`. Every crate has `#![deny(missing_docs)]`, so the public API is documented in
the code; this page explains how the parts fit.

The data folder of a connector is `<workspace>\connectors\<id>` (`location::resolve`). The
workspace is found, once at start, from `--workspace`, then the `STINTMATE_WORKSPACE`
environment variable, then the pointer file `%APPDATA%\Stintmate\settings.json`
(`platform::workspace`); a failure is an error, never a fallback, and the process exits with 3
(`WORKSPACE_ERROR_EXIT`). Logs go to `<data>\<name>.log` (`recorder.log`, `ui.log`,
`window.log`), capped at 5 MiB (`logging::DEFAULT_MAX_BYTES`).

## The control app

`control::run(spec, hooks, argv)` is the whole `main`. The command line (`control::cli`) has
`--tray`, `--managed --parent-pid <pid>`, `--register`, `--unregister`,
`--read-registration <out-file>`, `--quit`, `--stintmate-hold`, `--stintmate-release`,
`--from-shortcut`, `--version`, `--workspace` and `--allow-synced`; `cli::usage(spec)` prints
them all with the exit statuses. **Options it does not know are ignored**: they are kept in
`Args::ignored` and logged once, so a newer Stintmate can pass an option an older connector has
never heard of. A known option with a bad value is still an error (exit 2).

Inside the process:

- **`ControlApp`** (`control::app`) holds the state and does what the tray and the window ask,
  behind a small in-process API (`control::api`: commands in, snapshots and events out). A
  supervisor thread named `ui-ticker` ticks every second (`app::TICK`), or every 5 s while
  nothing runs and nothing is wanted (`app::IDLE_TICK`).
- **`ServiceManager`** (`control::services`) starts, stops, adopts and restarts the recorder, the
  only service (`control::SERVICES`). It spawns it suspended into a kill-on-close job object, so
  nothing the recorder starts can outlive the control app. A stop first signals the recorder's
  named stop event; a recorder that has not exited after 10 s (`STOP_TIMEOUT`) is terminated. A
  recorder that ends without a stop request is restarted after 2, 4, 8, 16 and 32 s
  (`RESTART_BACKOFF`); a run of 60 s (`STABLE_RUN`) resets the count. A recorder found running
  at start (started by hand) is adopted.
- **The endpoint** `connector-<id>` (`control::endpoint`) serves the control interface of the
  connector kit (`control.show`, `control.hide`, `control.quit`, `control.managed`,
  `control.service.start`, `control.service.stop`, `control.autostart.set`, `control.catalog.get`,
  `control.activity.act`), the state channels `control.state` and `control.activity` and the
  event channels `control.notices` and `control.services.desired`. The recorder connects to the
  same endpoint and declares `service.status`; the control app keeps the latest value per
  service. A service with no connection has no status.
- **The window API** (`ui.snapshot` state channel, `ui.command` method) is what the window
  process uses.
  level shown for the recorder (grey off, green ok, amber warn, red error;
  `resources::level_rgb`).
- **The single instance** (`control::instance`): a second launch does not start a second
  control app; it calls the running one through its endpoint.
- **Managed mode**: `--managed --parent-pid <pid>` runs without a tray icon for a managing app,
  and the process watches that parent (`managed::PARENT_CHECK`, every 2 s).
- **The descriptor** (`control::descriptor`): `--register` writes
  `%LOCALAPPDATA%\Stintmate\connectors\<id>.json` atomically (the folder can be replaced with
  `STINTMATE_CONNECTORS_DIR`). It carries `schema`, `id`, `product`, `name`, `sim`, `sim_name`,
  `version`, `launcher`, `args`, `control_protocol`, `endpoint`, `mutex`, `services`, `logs`,
  `window`, `features`, `written_at` and `written_by`, plus `flavour` and `uninstall` when given and
  `icon` (the absolute path of `icon.png` beside the launcher) when that file exists.
  `features` is the five the control app itself provides (`window`, `notices`,
  `translated_texts`, `remembered_services`, `options`, in `CONTROL_FEATURES`) followed by the
  spec's own, each once. `--register` refuses to overwrite a descriptor holding another
  `product`; `--unregister` removes it only when its `launcher` names this install and its
  `product` is this connector's.
- **Setups**: `--stintmate-hold` and `--stintmate-release` (`control::hold`) ask the Stintmate
  hub to stop this connector while an installer replaces its files, and to start it again.
- **The resume watcher** (`control::resume_watch`): see "Session resume" below.

## The window

`window::run(spec, argv)` takes `--page`, `--pipe`, `--key-file` and `--mutex` (the last three
are overrides for tests); any other option exits 2. It connects to the control app's endpoint,
draws `ui.snapshot` with egui (Services, Options and Logs pages, `window::gui`) and sends
`ui.command`. A second request to open it does not start another window: the control app
publishes the request in the snapshot (`window_request`) and the running window raises itself.
Exit codes: 0 after a normal close (or when a window is already open), 1 when it cannot
connect or draw, 2 for a bad command line or spec, 3 when the workspace cannot be resolved.

## The recorder's links

`link::boot::boot(spec, &args)` does the start-up in a fixed order: `--version` (prints
`<exe_prefix>-recorder <version>` and returns `Err(0)`), `spec.validate()` (`Err(2)`), the
workspace (`Err(3)`), the data folder, logging and the panic hook, the single-instance mutex
(`Err(1)` when another recorder runs), the stop event, below-normal process priority, the pipe
names, the key and the links. It returns a `Booted` with the location, the two links (not yet
started), the stop event, the `StatusPublisher` and the rate in Hz (`--hz`, else the spec's
`recorder_hz`). `RecorderArgs` knows `--workspace`, `--allow-synced`, `--hz`, `--pitwall-pipe`,
`--control-pipe` and `--version`; every other argument goes to `rest` for the connector's own
flags.

- **`PitwallLink`** is a client of the endpoint `pitwall`. It owns the reconnection (the IPC
  library's own is switched off), in a thread named `pitwall-link`. The state is `connected`,
  `waiting` (Pitwall is not running or the connection dropped) or `refused`. After a failed
  attempt it waits 0.25, 0.5, 1 and then 2 s, each with about +-20 % jitter (`link::backoff`,
  `link::jitter`); after a refusal (`unauthorized`, `bad_request`, `busy`, `connector_busy`) it waits 10 s
  (`REFUSED_RETRY`); after `connector_id_taken` it waits 60 s (`TAKEN_RETRY`), because only the
  user can change which product owns an id. Its hello carries `connector_id`, `connector_name`,
  `sim`, `sim_name`, `service` and `product` (`link::peer_info`), all from the spec.
- **`ControlLink`** is a client of the connector's own endpoint. It reconnects by itself and a
  missing control app is normal.
- **`StatusPublisher`** sets the `connector.status` and `service.status` channels, only when the
  value changed. **`StatusWorker`** (`link::status`) runs the publishing on its own thread
  (`status`): the connector's loop fills a preallocated `StatusInput` and hands it over only
  when it differs from the last one, so the loop does no allocation for it.
- **`Announcer`** (`link::announcer`) declares, at every new connection, the connector's
  announcement catalog, its guidance and its actions, and sends `pitwall.announce` without
  waiting. It keeps the rate windows itself, 10 per 60 s and 120 per 60 s for the `spotter`
  level (`wire::rate`), and drops, never queues, when Pitwall is away or the limit is reached.

## The outbox

Every write to Pitwall goes to disk first (`link::outbox`). `Outbox::put` stores it as
`<20-digit sequence>.msgpack` (written to a temporary file, then renamed) holding the method,
the idempotency key and the params. A sender thread named `outbox` sends the files in sequence
order, one at a time, waiting up to 30 s (`SEND_TIMEOUT`) for each answer. `ok` deletes the file.
A non-retryable error (`bad_request`, `session_closed`, `unknown_session`, `too_large`,
`unknown_method`, `version_unsupported`; `NON_RETRYABLE`) moves it to the `rejected` folder.
Anything else keeps the file and sends it again with the same idempotency key after the
`retry_ms` the answer carried, 1 s by default, kept between 50 ms and 10 s. At start the outbox loads what
an earlier run left, deletes `.tmp` files (writes cut short by a crash), and continues the
sequence after the highest number on disk. So laps finished while Pitwall is not running are
kept and sent, in order, when it is.

## The sink: sessions, keys and resume

`WriteSink` (`link::sink`) is the generic writer of a connector. The connector hands it ready
messages, or closures that build them, and it does the rest on its own thread named
`pitwall-writer`; the sampling loop never waits for it.

- A session is named by a **session key**, 16 random hex digits (`new_session_key`) chosen when
  the connector opens it (`start_session`).
- Every write gets an **idempotency key**, assigned by the sink and stored with the write:
  `<prefix>/<session_key>/start`, `.../lap/<seq>`, `.../setup/<seq>`, `.../pit_exit/<seq>`,
  `.../participants/<seq>` and `.../end`, and `<prefix>/catalog/<12 hex digits of the SHA-256>`
  for a catalog. The prefix is the connector's id. Sending the same write again is therefore
  harmless.
- **Laps and setups are built by closures** (`LapBuild`, `SetupBuild`) that run on the writer
  thread, so the loop only moves the data it already has.
- **Session resume.** A recorder stopped while the game runs does not end its session. It
  keeps the key and the sequences in `session-resume.json`, a sibling of the outbox folder.
  A recorder started later whose first session has the same identity string (what must be equal:
  car, track and so on, joined by the connector in `SessionOpen::identity`), the same game
  token (`set_game_token`) and whose game clock continued (`ResumeClock::continues_as`)
  continues the session under the same key. Anything else gets a new key. The connector
  supplies the `ResumeClock` type: how its clock is stored, read back and compared.
- **The resume watcher** in the control app deletes that file when it no longer applies. About
  once a second, while the file exists and no recorder runs, it calls the connector's
  `ResumeProbe::look(stored_clock, game_running)`, which answers `Gone`, `NoReading`, `Keep`
  or `Drop`; the file is deleted on `Gone` and `Drop`. The default probe, `NoResume`, answers
  `Gone` when the game is not running and `NoReading` otherwise.

## Texts and catalogs

The English texts belong to the connector: it embeds its own catalog file (the text of
`ConnectorSpec::ui_catalog`) with the catalog id `<id>-connector`. `i18n::Embedded::parse`
checks the schema and the id and computes `rev`, the first 12 hex digits of the SHA-256 of the
canonical JSON. `control::texts::Texts::load` reads the `language` of
`<workspace>\hub\settings.json` and lays the workspace's language packs over the English per key
(`i18n::load`, packs under `<workspace>/locales/<tag>/<catalog id>.json`); a pack file that is
invalid is skipped and logged. `control.catalog.get` returns the embedded document and its
`rev`. Logs, JSON files, IPC error texts and ids are not localised. The pseudo-locale `qps`
(`i18n::pseudo_pack`) wraps every text in `⟦…⟧` so a test can find a visible text that did not
go through the catalog.

`control::texts::REQUIRED_KEYS` lists every key the control app and the window use with the
placeholders each takes, and `check_ui_catalog` checks a connector's catalog against it.

The announcement catalog (what the engineer may say) is a different file, in the format of the
connector kit's `catalogs.md`: `wire::catalog::AnnounceCatalog::parse(json, expected_id)`
validates it, and its idempotency key is `<prefix>/textcatalog/<12 hex digits>`.

## The development tag

`STINTMATE_DEV_INSTANCE=<tag>` (`platform::dev`) makes a second copy of a connector that
shares nothing with the installed one. It is read in `platform::dev` and nowhere else; then
each name in the spec's `dev_env_aliases` is tried. The tag is lower-cased, anything but
`a-z0-9` becomes `-`, and at most 24 characters are kept. When set, every kernel object name
appends it (`Local\<mutex_prefix>.UI.<tag>`, `.Window.<tag>`, `.Recorder.<tag>`,
`.Recorder.Stop.<tag>`; `single::Names::for_spec`) and every pipe uses the pipe user
`<user>-<tag>`, for the connector's own endpoint and for the Pitwall pipe it dials
(`dev::pipe_path`). Production never sets it.

## What the connector supplies

Everything that reads the simulator stays in the connector: the shared-memory or API reader,
the tracker that turns frames into sessions and laps, the channels and names, the setup
mapping, the telemetry stream (`wire::telemetry::Sample` is the format), the live frame and the
live server, and its development tools. Through the runtime it supplies:

- one `static ConnectorSpec` and its three `main` functions;
- its English UI catalog and its announcement catalog;
- a `ResumeProbe` for the control app (`Hooks`) and a `ResumeClock` for the sink;
- the code that opens sessions and builds laps for `WriteSink`;
- the loop that fills the `StatusInput`.

[embedding.md](embedding.md) goes through each of these.

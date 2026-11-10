# Changelog

All notable changes to this project are written here.

## 0.3.0 (2026-11-11)

**Breaking**: the tyre-reading types of 0.2.1 are removed, so the minor version moves. That message was replaced in the connector kit before any connector shipped it by a generic one for data the game gives at a moment; use `WriteSink::sim_data` and `WriteSink::sim_data_kinds`.

- **Removed.** `stintmate_connector_wire::messages::TyreReadings`, `TyreWheels` and `TyreWheel`, and `WriteSink::tyre_readings`. The feature `tyre_readings` is no longer in `spec::FEATURES`, and the fake Pitwall of `stintmate-connector-testing` no longer answers `pitwall.write.tyre_readings`.
- **Simulator data.** Two write messages, from connector kit 0.6.0. `messages::SimData { taken_at, kind, data }` (`data` is a `serde_json::Map`) is a record of a structure the connector designs, stored as sent; `messages::SimDataKinds { kinds }`, with `SimDataKind { kind, description, topic, fields }` and `SimDataField { meaning, unit }`, describes each kind field by field so the race engineer reads the records without guessing (a field path is dotted, a `*` segment stands for any key or array index). A kind's fields never change meaning: a changed meaning is a new kind name.
- **`WriteSink::sim_data(local, kind, taken_at, data)`**, sent as `pitwall.write.sim_data` with the idempotency key `<prefix>/<session_key>/sim_data/<seq>`; one sequence per session for all kinds. Send every lap that ended before the data first, and a stop's data before its `pit_exit`.
- **`WriteSink::sim_data_kinds(kinds)`**, sent as `pitwall.write.sim_data_kinds` with the key `<prefix>/sim_data_kinds/<sha12 of the canonical params>` (the catalog's rule). The sink keeps the last description given and queues it again before every `session_start` it sends, so Pitwall (which writes a content once) always has the descriptions of the session it receives.
- **Local checks.** The new module `stintmate_connector_wire::sim_data` holds the limits as public constants (data of at most 16,384 bytes of compact JSON, 6 nested levels, 512 keys in all, 256 items per array, 500 characters per string, safe integers, 5,000 records per session, 32 kinds, 64 fields per kind) and `check_data(kind, &Map)` and `check_kinds(&[SimDataKind])`, with the texts of Pitwall's error replies. Both sink calls use them: a record or description that breaks a limit is logged at error level with the reason and dropped, and so is the 5,001st record of a session.
- **Feature.** `spec::FEATURES` has `sim_data` (the kit's feature vocabulary) in place of `tyre_readings`.
- **Resume file.** `session-resume.json` keeps five write sequences; the fifth is now `sim_data`'s. A file written by 0.2.1 or 0.2.2, whose fifth counted tyre readings, hands that value to `sim_data` (keys only need to be unique); a file with four items reads the fifth as 0.
- **Needs a Pitwall that implements simulator data.** An older one answers `unknown_method` to both messages; the outbox drops those writes without retrying.
- **Testing.** The fake Pitwall answers `pitwall.write.sim_data` and `pitwall.write.sim_data_kinds`. The schema fixture of the wire tests is the kit's write API schema of 0.6.0.

To migrate, replace each `tyre_readings(local, r)` with a `sim_data` kind of your connector: describe it once with `sim_data_kinds` and send each record with `sim_data`. Declare the feature `sim_data` instead of `tyre_readings`.

## 0.2.2 (2026-11-10)

- **The feature `tyre_readings`.** `FEATURES` (the contract vocabulary a connector's declared features are checked against) gains `tyre_readings`, from connector-kit 0.5.0, so a connector that sends `tyre_readings` can declare it. No other change.

## 0.2.1 (2026-11-10)

Additive; no API change for existing code.

- **Tyre readings.** A new write message, `tyre_readings`, for readings the simulator gives at a moment rather than as live telemetry: when the car enters its pit box, when the simulator puts it back in the pits, when the driver gets into the car, after a garage visit. `stintmate-connector-wire` gains `messages::TyreReadings { taken_at, occasion, wheels }`, `TyreWheels { fl, fr, rl, rr }` (each an `Option<TyreWheel>`, a missing wheel is left out) and `TyreWheel` (carcass and surface temperatures and tread remaining at the inner, middle and outer edge, and hot pressure; every value an `Option<f64>`, tread `1.0` is new). `stintmate-connector-link` gains `WriteSink::tyre_readings(local, readings)`, sent as `pitwall.write.tyre_readings` with the idempotency key `<prefix>/<session_key>/tyre_readings/<seq>`; wheels without a finite value are dropped and nothing is queued when no reading is left. Send the laps that ended before the readings first, and a stop's readings before its `pit_exit`.
- **Needs a newer Pitwall.** The message is part of write API 0.5.0 of the connector kit (the test schema is updated to it). A Pitwall that does not know it answers `unknown_method`; the outbox drops that write without retrying and goes on with the next (now covered by a test).
- **Resume file.** `session-resume.json` stores five write sequences instead of four. A file written by 0.2.0 or older, with four, is read with the fifth at 0.
- The sink no longer maps an unknown write kind onto the rival-laps counter; it is logged and dropped.
- **Testing.** The fake Pitwall of `stintmate-connector-testing` answers `pitwall.write.tyre_readings`.

## 0.2.0 (2026-11-10)

Breaking for code that builds a `ConnectorSpec` with a struct literal (a new public field), so the minor version moves.

- **Connector artwork.** `ConnectorSpec` has a new field `icon: Option<ConnectorIcon>`; set it to `None` to keep the glyph icons exactly as before, or to `Some(ConnectorIcon { png_32, png_64 })` (two 8-bit RGBA PNG files, usually `include_bytes!`) to use your own artwork. The tray shows it with a badge of the connector's level (grey off, green ok, amber warn, red error) in the lower right corner; the window shows it alone. A file that does not decode is logged and the glyph is drawn instead. `stintmate-connector-resources` gains `ConnectorIcon` and, behind the new `artwork` feature (on in the control and window crates), `ConnectorIcon::rgba`, `ConnectorIcon::badged_rgba`, `ArtworkError`, `TRAY_SIZE` and `WINDOW_SIZE`; that crate stays standard-library only without the feature. The executables' `.ico` needs nothing new: pass the path of a checked-in file to `version_rc`. New dependency: `png` 0.18 (MIT or Apache-2.0).

  To migrate, add `icon: None,` to every `ConnectorSpec { .. }` literal.
- **CI on pull requests.** The `ci` workflow runs on every pull request to `main` (and by hand). No code change.

## 0.1.3+docs.3 (2026-11-10)

- **Hooks.** `.githooks/` runs `cargo fmt --all --check` before a commit and clippy before a push
  (enabled with `git config core.hooksPath .githooks`). No code change.

## 0.1.3+docs.2 (2026-11-09)

- **Documentation.** A second pass against the code; behaviour unchanged. The architecture page now lists the descriptor's optional `icon` key and no longer names a private constant as if it were public.

## 0.1.3+docs.1 (2026-11-07)

- **Documentation.** Checked against the code; corrections: the status version, the release dated, the refusal codes of the link, the IPC tag line of the embedding guide.

## 0.1.3 (2026-11-07)

Additive; no API change.

- `stintmate-connector-control`: the descriptor carries the optional key `icon`, the absolute path
  of `icon.png` in the folder of the control app's executable. It is written only when that file
  exists, so a connector whose setup installs the PNG next to its executables gets the icon on
  registration and one without it is unchanged.

## 0.1.2 (2026-11-05)

- `stintmate-connector-testing`: `EXAMPLE_SPEC` takes its version from the crate, so the example
  executables report the release they were built from. A test fix; behaviour unchanged.

## 0.1.1 (2026-11-05)

Behaviour changed; the protocol is unchanged.

- `stintmate-connector-link`: a `bye` with `unauthorized`, `bad_request`, `busy`, `connector_busy`
  or `connector_id_taken` is a refusal whether it arrives during the handshake or after the hello:
  the link state is `refused`, with the wait of `REFUSED_RETRY` (`TAKEN_RETRY` after
  `connector_id_taken`). Before, a `bye` after the hello was taken for a lost connection (state
  `waiting`, retry after the short backoff). A lost connection without a `bye`, or a `bye` with
  another code, stays `waiting`.
- `stintmate-connector-platform`: `proc::lower_process_priority` returns whether the class was
  set, and `proc::process_priority_class` reads it back.
- `stintmate-connector-control`, `stintmate-connector-window`: each process lowers its own
  priority to below normal at start and logs it once, so the control app and the window are below
  normal also when they are not started by the control app. The recorder already did.

## 0.1.0 (2026-11-05)

First release. The control app, window, recorder links, outbox, sink, status, texts, write-API structs, catalog validation and telemetry sample of the AMS2 connector 3.4.0, with the connector named by a `ConnectorSpec`. New against that code: the control app ignores unknown launcher options; `STINTMATE_DEV_INSTANCE` separates every process; the descriptor carries `window` and `features`.

- `stintmate-connector-control`: the launcher ignores options it does not know (they are kept in
  `Args::ignored` and logged once) and a value after one is not read as a known option; known
  options keep their errors. The descriptor gains `window` and `features` (the five the control
  app gives, then the connector's own). `texts::REQUIRED_KEYS` and `texts::check_ui_catalog` check
  a connector's English UI catalog. The command-line, endpoint (with the `stintmate-ipc-check
  server` conformance run), hold and service tests run against the example connector.
- `stintmate-connector-testing`: `EXAMPLE_SPEC` carries an example UI catalog.
- `stintmate-connector-link`: `WriteSink::catalog` queues a catalog write in the middle of a
  session, for cars first seen after it opened.
- `stintmate-connector-window`: the settings window of a connector (Services, Options and Logs pages, the dark theme and the bundled Barlow, Barlow Condensed and JetBrains Mono fonts under the SIL Open Font License). `run(spec, argv)` is the whole `main` of the window process: its names, version, icon glyph and texts come from the connector spec and its catalog; `--page`, `--pipe`, `--key-file` and `--mutex` are the options, anything else exits 2. Tests render every page offscreen to PNG files under `target/tmp/window-renders`.

- `stintmate-connector-link`: `sink::WriteSink`, the generic writer of a connector: session keys,
  idempotency keys under the connector id, write sequences, the session resume file (identity
  string and a `ResumeClock` the connector supplies), the stint Pitwall answered and the
  row-by-row requeue of a refused rival batch. Laps and setups are built by closures that run on
  the writer thread.

- `stintmate-connector-link`: the recorder's plumbing: `link` (the Pitwall and control links,
  the hello from the connector spec, the backoff and the waits after a refusal), `outbox`,
  `status`, `announcer` (catalog, rate windows and declarations take the connector's catalog and
  id), `clock` and `boot` (arguments and start-up steps, with the exit codes).
- `stintmate-connector-testing`: `FakePitwall`, temporary workspaces with a test key, unique pipe
  users and an example connector spec with its announcement catalog.

- The text catalog test fixtures are kept byte for byte (line endings included), so their pinned hashes hold on every checkout.
- Repository created.
- Cargo workspace with the toolchain pin, the static C runtime setting, `deny.toml`, `NOTICE`,
  `SECURITY.md` and a manually started CI workflow.
- `stintmate-connector-resources`: the connector icons drawn in code (`Glyph::A`, `Glyph::I`,
  `level_rgb`, `render_rgba`, `ico_bytes`) and the Windows resource script (`version_rc`).
- `stintmate-connector-platform`: the workspace resolver, logging, single-instance mutex and stop
  event, process, window, game and timer helpers, and `ConnectorSpec` / `GameSpec` with their
  validation. The development tag is read from `STINTMATE_DEV_INSTANCE`, then from the spec's
  aliases; tagged mutex names append the tag (`Local\<prefix>.UI.<tag>`).
- `stintmate-connector-i18n`: the text catalogs contract loader (`load` with the catalog id,
  `Catalog`, plural rules, validation, pseudo-locale) and `Embedded`, which holds a connector's
  embedded English catalog after checking its schema and catalog id. The contract's vectors are
  tested against it.
- `stintmate-connector-wire`: the write API message structs (`LapChannels` takes owned or static
  channel names), the 32-byte `connector.telemetry` sample with its flags, the announcement
  catalog format (`AnnounceCatalog`, `validate_catalog`, canonical JSON, `sha12`, source hashes,
  typed params), the announcement rate window, `standard_actions`, and the test schema checker
  with the connector kit's write API and protocol schemas. Every message is validated against
  the write API schema in the tests.

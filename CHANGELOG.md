# Changelog

All notable changes to this project are written here.

## 0.1.3 (unreleased)

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

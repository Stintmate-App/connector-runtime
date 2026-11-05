# connector-runtime

The shared Rust runtime of [Stintmate](https://stintmate.app)'s official simulator connectors:
the platform helpers, the texts, the tray control app, the settings window, the recorder's links
to Pitwall (Stintmate's telemetry database), its outbox and sink, the write API messages and
the validation of announcement catalogs. A connector supplies what is specific to its
simulator; this library does the rest.

**Status:** release 0.1.0. It is made from the code of the AMS2 connector 3.4.0 with the
simulator taken out. Everything that depends on the simulator comes in through one value, a
[`ConnectorSpec`](#the-connector-spec).

## What it is, and what it is not

- It is a Cargo workspace of **library crates** for Windows (`x86_64-pc-windows-msvc`). A
  connector is three small executables that call into it.
- It ships **no executable and no installer**. Each connector owns its three `main.rs` files, its
  `build.rs`, its installer and the code that reads its simulator.
- It does **not** read a game. The tracker, the telemetry stream and everything else that
  touches the simulator stay in the connector.
- It does **not** define the protocol. The contract between a connector and Stintmate (the
  connector protocol, the write API, the descriptor, the catalogs format) is the
  [connector kit](https://github.com/Stintmate-App/connector-kit)'s, and the transport is
  [Stintmate IPC](https://github.com/Stintmate-App/stintmate-ipc). This library implements the
  connector side of them.

## The crates

| Crate | What |
|---|---|
| `stintmate-connector-resources` (`crates/resources/`) | the connector icons drawn in code (`Glyph`, `render_rgba`, `ico_bytes`) and the Windows version resource script (`version_rc`); standard library only, so a `build.rs` can use it |
| `stintmate-connector-platform` (`crates/platform/`) | `ConnectorSpec` and its validation, the workspace resolver, logging, the single-instance mutex and stop event, the development tag, process, window, game and timer helpers |
| `stintmate-connector-i18n` (`crates/i18n/`) | the text catalogs loader: language packs over the connector's embedded English catalog, plural rules, validation, the pseudo-locale |
| `stintmate-connector-wire` (`crates/wire/`) | the write API message structs, the 32-byte `connector.telemetry` sample, the announcement catalog format and its validation, the announcement rate window, the standard actions, a schema checker for tests |
| `stintmate-connector-link` (`crates/link/`) | the recorder's plumbing: the links to Pitwall and to the control app, the outbox, `WriteSink`, the status worker, the announcer and `boot` (the start-up steps and the command line) |
| `stintmate-connector-control` (`crates/control/`) | the control app as a library: command line, tray, supervision of the recorder, the connector's own endpoint, the descriptor, autostart, the single instance |
| `stintmate-connector-window` (`crates/window/`) | the settings window (Services, Options and Logs pages), a short-lived process the control app starts on demand; the only crate that uses egui |
| `stintmate-connector-testing` (`crates/testing/`) | test support: a fake Pitwall on a unique pipe, temporary workspaces with a test key, an example `ConnectorSpec`; a dev-dependency only |

Documentation: [`docs/`](docs/README.md): [architecture](docs/architecture.md) and
[writing a connector on the runtime](docs/embedding.md).

## Using it

All crates are in this repository and are not published on crates.io (`publish = false`).
Depend on them as git dependencies pinned to a tag, and on the IPC library at the same tag as
this repository's `Cargo.toml` (v0.3.3 for this release):

```toml
[dependencies]
stintmate-connector-platform  = { git = "https://github.com/Stintmate-App/connector-runtime", tag = "v0.1.0" }
stintmate-connector-control   = { git = "https://github.com/Stintmate-App/connector-runtime", tag = "v0.1.0" }
stintmate-connector-link      = { git = "https://github.com/Stintmate-App/connector-runtime", tag = "v0.1.0" }
stintmate-connector-wire      = { git = "https://github.com/Stintmate-App/connector-runtime", tag = "v0.1.0" }
stintmate-connector-window    = { git = "https://github.com/Stintmate-App/connector-runtime", tag = "v0.1.0" }
stintmate-ipc                 = { git = "https://github.com/Stintmate-App/stintmate-ipc", tag = "v0.3.3" }

[dev-dependencies]
stintmate-connector-testing   = { git = "https://github.com/Stintmate-App/connector-runtime", tag = "v0.1.0" }

[build-dependencies]
stintmate-connector-resources = { git = "https://github.com/Stintmate-App/connector-runtime", tag = "v0.1.0" }
```

Each crate is used by the executable that needs it, so the tray process never links egui
(only the window crate does) and the recorder never links the tray library. Sketches of the
three `main.rs` files of a connector (`SPEC` is the connector's `static ConnectorSpec`):

```rust
// <exe_prefix>-connector-ui.exe: the control app (tray, supervision of the recorder)
use stintmate_connector_control::{Hooks, run};

fn main() {
    let argv = std::env::args().skip(1).collect();
    std::process::exit(run(&SPEC, Hooks::default(), argv));
}
```

```rust
// <exe_prefix>-connector-window.exe: the settings window
fn main() {
    let argv = std::env::args().skip(1).collect();
    std::process::exit(stintmate_connector_window::run(&SPEC, argv));
}
```

```rust
// <exe_prefix>-recorder.exe: the recorder
use stintmate_connector_link::boot::{RecorderArgs, boot};

fn main() {
    let args = match RecorderArgs::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(line) => {
            eprintln!("{line}");
            std::process::exit(2);
        }
    };
    let booted = match boot(&SPEC, &args) {
        Ok(booted) => booted,
        Err(code) => std::process::exit(code),
    };
    booted.pitwall.start();
    if let Some(control) = &booted.control {
        control.start();
    }
    // The connector's own loop: read the simulator at `booted.hz`, hand the laps to a
    // `WriteSink`, publish the status, and stop when `booted.stop.is_set()`.
}
```

### The connector spec

A connector declares one `static` [`ConnectorSpec`] and passes it as `&'static ConnectorSpec` to
every entry point; there is no global. It holds the identity (`id`, `name`, `sim`, `sim_name`,
`product`, `version`), the names derived from it (`exe_prefix`, `mutex_prefix`,
`autostart_value`), how to recognise the game (`GameSpec`: image names, an optional window
title and class, an optional cheap "the game may run" hint), the default recorder rate, the
features the descriptor declares, the embedded English UI catalog, the icon glyph and the
environment variables that also set the development tag. `ConnectorSpec::validate` checks them;
the control app, the window and `boot` call it first and exit with 2 if it fails. The field
by field description, the build script and the tests to copy are in
[`docs/embedding.md`](docs/embedding.md).

[`ConnectorSpec`]: crates/platform/src/spec.rs

## Building

Windows only. Requirements: Rust through [rustup](https://rustup.rs) (`rust-toolchain.toml` pins
the toolchain, 1.99, with `rustfmt`, `clippy` and the `x86_64-pc-windows-msvc` target, and rustup
installs it on the first `cargo` call), the Visual Studio Build Tools C++ workload (MSVC linker)
and git: the Stintmate IPC library is fetched by cargo from its repository (tag in
`Cargo.toml`).

```
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check licenses advisories bans sources
```

The last command needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) (`deny.toml`).
The window crate's lifecycle test opens a real window and needs a desktop with OpenGL; set
`STINTMATE_NO_DESKTOP=1` to skip it. `.cargo/config.toml` links the C runtime statically; it
applies to this repository's own builds, and each connector sets its own.

## Versioning

Versions are semantic (`MAJOR.MINOR.PATCH`) and are those of the whole workspace: every crate
has the version in the root `Cargo.toml`. A release is a git tag `vX.Y.Z` on `main`, and a
connector depends on the tag, never on a branch. [`CHANGELOG.md`](CHANGELOG.md) lists every
change.

## Contributing

Issues are welcome. Pull requests are welcome too, but there is no promise that they are
reviewed or merged. For a larger change, open an issue first. Report a security problem
privately, as described in [`SECURITY.md`](SECURITY.md).

## Licence and name

Apache License 2.0 ([`LICENSE`](LICENSE), [`NOTICE`](NOTICE)). Every dependency is permissively
licensed (checked by `cargo deny`, [`deny.toml`](deny.toml)); the window crate bundles the fonts
Barlow, Barlow Condensed and JetBrains Mono under the SIL Open Font License.

"Stintmate" is a name of the Stintmate project; the licence grants no right to use it except to
describe the origin of the work. Forks are welcome; please give them another name.

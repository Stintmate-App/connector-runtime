//! The control app of a Stintmate connector, as a library: everything but the game.
//!
//! A connector's control executable is `fn main` calling [`run`] with its
//! [`ConnectorSpec`] and its [`Hooks`].
//!
//! * [`app`]: [`app::ControlApp`], the heart: what the tray and the window show and do, behind
//!   a small in-process API ([`api`]: commands in, snapshots and events out), so the UI can
//!   live in this process or in a separate window process;
//! * [`endpoint`]: the control endpoint `connector-<id>` (control interface, `control.state`,
//!   `control.activity`, `control.notices`, `control.services.desired`, the services'
//!   `service.status`);
//! * [`services`]: the ServiceManager (spawn suspended into a kill-on-close job, adopt, clean
//!   and forced stop, restart with backoff) and [`status`], the states and labels;
//! * [`config`], [`descriptor`], [`autostart`], [`instance`] (single instance and the second
//!   launch's call), [`managed`] (parent watch), [`logtail`], [`timefmt`], [`texts`];
//! * [`resume_watch`]: the kept-open session's watcher and the [`ResumeProbe`] a connector
//!   supplies.
#![deny(missing_docs)]

pub mod api;
pub mod app;
pub mod autostart;
pub mod cli;
pub mod config;
pub mod descriptor;
#[cfg(windows)]
pub mod dialog;
pub mod endpoint;
pub mod hold;
pub mod instance;
pub mod logtail;
pub mod managed;
pub mod resume_watch;
pub mod services;
pub mod shortcut;
pub mod status;
pub mod texts;
pub mod timefmt;
#[cfg(windows)]
pub mod tray;
pub mod tray_model;
pub mod window_host;

#[cfg(test)]
mod testspec;

pub use resume_watch::{NoResume, ResumeLook, ResumeProbe};

use std::sync::Arc;
use stintmate_connector_platform::spec::ConnectorSpec;

/// The services `control.service.start|stop` take.
pub const SERVICES: [&str; 1] = ["recorder"];

/// What a connector adds to the control app: the parts that read the game.
pub struct Hooks {
    /// The look at the game for a kept-open session (see [`resume_watch`]).
    pub resume: Arc<dyn ResumeProbe>,
}

impl Default for Hooks {
    fn default() -> Self {
        Self {
            resume: Arc::new(NoResume),
        }
    }
}

/// Runs the control app for `spec` with the command line `argv` (the program name excluded) and
/// returns the process exit code.
pub fn run(spec: &'static ConnectorSpec, hooks: Hooks, argv: Vec<String>) -> i32 {
    cli::main(spec, hooks, argv)
}

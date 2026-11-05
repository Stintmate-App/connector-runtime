//! The settings window of a Stintmate connector: a separate, short-lived process
//! started by the control app when the user opens the window and gone when it closes. It
//! draws the control app's `ui.snapshot` and sends `ui.command` over the connector's endpoint;
//! the tray process never loads egui or a graphics context.
//!
//! * [`run`]: the whole `main` of the window process, given the connector's spec;
//! * [`link`]: the endpoint client (the snapshot in, commands out);
//! * [`gui`]: the egui application (Services, Options and Logs pages);
//! * [`theme`]: the palette, the bundled fonts and the painted widgets.

#![deny(missing_docs)]

pub mod gui;
pub mod link;
pub mod theme;

mod launch;

pub use launch::run;

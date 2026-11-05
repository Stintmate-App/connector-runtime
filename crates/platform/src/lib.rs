//! Windows helpers shared by the executables of a Stintmate connector.
//!
//! Everything that depends on the simulator comes in through one value, a
//! [`spec::ConnectorSpec`].
//!
//! - [`spec`]: the connector's identity (names, product key, game) and its validation;
//! - [`workspace`]: the Stintmate workspace resolver (Stintmate IPC SPEC section 4.3) and [`location`], the
//!   connector's data folder;
//! - [`logging`]: `tracing` to `<data>\<name>.log`, the line format `date time LEVEL target: message`,
//!   non-blocking, with a size cap;
//! - [`dev`]: the development-only instance tag (`STINTMATE_DEV_INSTANCE`): names and pipes;
//! - [`single`]: the named mutex (single instance) and the named stop event;
//! - [`procinfo`]: is a PID alive, and its process chain (for logs);
//! - [`proc`]: process helpers (image name, start time, kill-on-close job object, spawn
//!   suspended) and [`game`], the "is the game running" check;
//! - [`window`]: which window is the game window;
//! - [`timer`]: the high-resolution waitable timer for the recorder's loop.
#![deny(missing_docs)]

pub mod dev;
pub mod location;
pub mod logging;
pub mod spec;
pub mod workspace;

#[cfg(windows)]
pub mod console;
#[cfg(windows)]
pub mod game;
#[cfg(windows)]
pub mod proc;
#[cfg(windows)]
pub mod procinfo;
#[cfg(windows)]
pub mod single;
#[cfg(windows)]
pub mod timer;
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub mod window;

#[cfg(windows)]
pub use win::{OwnedHandle, Win32Error};

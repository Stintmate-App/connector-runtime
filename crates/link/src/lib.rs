//! The plumbing of a connector's recorder process, free of any simulator.
//!
//! * [`link`]: the connection to Pitwall ([`link::PitwallLink`], with its own reconnection and
//!   the waits after a refusal), the connection to the connector's control app, the hello
//!   ([`link::peer_info`]) and the publisher of `connector.status` / `service.status`.
//! * [`outbox`]: every write to Pitwall goes through the disk first and is sent in order.
//! * [`sink`]: session keys, write sequences, idempotency keys and the session resume file.
//! * [`status`]: the status the loop reports, and the thread that publishes it.
//! * [`announcer`]: announcements, rate windows and the per-connection declarations.
//! * [`clock`]: the time source.
//! * [`boot`]: the recorder's start-up steps and command line.
//!
//! Everything that depends on the simulator comes from a
//! [`ConnectorSpec`](stintmate_connector_platform::spec::ConnectorSpec) or from the caller.

#![deny(missing_docs)]

pub mod announcer;
pub mod boot;
pub mod clock;
pub mod link;
pub mod outbox;
pub mod sink;
pub mod status;

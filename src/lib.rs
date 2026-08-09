//! llama-tray: a panel control for the local `llama-server` user service.
//!
//! The 5090 cannot hold a 27B model and a game at the same time, so the server
//! has to come down before playing and back up afterwards. This puts that
//! toggle, and enough numbers to know what the card is doing, in the panel.
//!
//! # Shape
//!
//! - [`server`] is the only module that talks to anything outside the process:
//!   systemd over the session bus, `llama-server` over a socket, `nvidia-smi`
//!   over a pipe. Every call returns a typed outcome; nothing panics on a
//!   server that is simply not running.
//! - [`status`] is pure. It turns a [`status::Snapshot`] into the rows of the
//!   menu and has no idea where the numbers came from.
//! - [`tray`] is the StatusNotifierItem transport, adapted from `stickies`.

pub mod lifetime;
pub mod server;
pub mod status;
pub mod tray;

/// Reverse-DNS id, used for the bus name and the icon.
pub const APP_ID: &str = "us.hagreli.LlamaTray";

/// The unit this controls. Not configurable: the whole app is about this one
/// service, and a wrong name here would present a permanently dead toggle.
pub const UNIT: &str = "llama-server.service";

/// Where the server listens, matching `--host`/`--port` in `llama-qwen3`.
pub const ENDPOINT: &str = "127.0.0.1:8080";

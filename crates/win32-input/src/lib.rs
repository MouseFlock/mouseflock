//! Low-level hooks, Raw Input, `SendInput`, cursor warp, DPI.
//!
//! One of two crates where `unsafe` is allowed. Every `unsafe` block carries a
//! `// SAFETY:` comment. Hook procedures do nothing but enqueue: Windows
//! silently unhooks a slow low-level hook. Linked only by `bin/agent`.
//!
//! # Use
//!
//! [`start`] installs the hooks and returns a [`Capture`] and a receiver of
//! [`Captured`] events: local input with its time and [`Origin`], and the
//! [`Screen`] whenever it changes. [`Capture::set_mode`] decides which
//! physical input the hooks keep from the OS; [`Capture::inject`] and
//! [`Capture::warp`] carry out the engine's decisions. Dropping the
//! [`Capture`] returns to [`CaptureMode::PassAll`] and removes the hooks.
//!
//! # Threads
//!
//! The input thread owns both low-level hooks and the Raw Input window and
//! does nothing else, so a hook call never waits behind slow work. The
//! screen thread enumerates monitors on display-change messages and every
//! 2 seconds, and reports only a screen that differs from the last one.
//! With [`Options::report_input_desktop`] it also checks every 50 ms
//! whether the process's desktop is the input desktop, for the winlogon
//! agent on the secure desktop.
//!
//! # Origin
//!
//! Keys, buttons and wheel take their origin from the hooks' injected flags.
//! Motion comes from Raw Input, before pointer speed: input carrying
//! DeskHop's tag ([`decide::TAG`]) is injected, input with a device handle
//! is physical, and anything else follows the last move the mouse hook saw.

mod capture;
mod clock;
mod decide;
mod desktop;
mod edid;
mod inject;
mod keys;
mod screen;

use model::{InputEvent, Millis, Origin, Screen};

pub use capture::{Capture, Options, StartError, start};
pub use clock::now;
pub use edid::Edid;
pub use inject::{InjectError, WarpError};
pub use model::CaptureMode;
pub use screen::{MonitorDetail, TargetDetail};

/// Something the capture observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Captured {
    /// Local input, in the order the OS delivered it.
    Input {
        at: Millis,
        input: InputEvent,
        origin: Origin,
    },
    /// The monitors, at start and whenever they change, with the details
    /// behind each monitor's identity for diagnostics.
    Screen(Screen, Vec<MonitorDetail>),
    /// Whether the process's desktop is the input desktop, with
    /// [`Options::report_input_desktop`]: right after the first screen, then
    /// on each change.
    InputDesktop(bool),
}

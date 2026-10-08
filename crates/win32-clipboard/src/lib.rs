//! Clipboard listener, and Unicode text and images in and out.
//!
//! One of four crates where `unsafe` is allowed. Every `unsafe` block carries
//! a `// SAFETY:` comment. Linked only by `bin/agent`.
//!
//! - [`text`]: the rules without the clipboard: privacy markers and UTF-16
//!   to UTF-8 within the 4 MiB limit.
//! - [`image`]: bitmaps to PNG and back, as pure functions, with the image
//!   limits.
//! - [`start`]: the `deskhop-clipboard` thread, which reports each settled
//!   copy and sets text without reporting it back.

pub mod image;
mod listener;
pub mod text;

pub use listener::{Clipboard, Copied, SETTLE_MS, start};

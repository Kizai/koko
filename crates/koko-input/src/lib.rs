//! Cross-platform input support. macOS input is posted by the Swift app.

pub mod coordinates;
pub mod keyboard;
pub mod scroll;

#[cfg(any(windows, test))]
pub mod windows;

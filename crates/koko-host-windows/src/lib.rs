//! Local application interface to the bundled daemon. Pairing, scope and session
//! enforcement stay in the daemon; the desktop backend owns the local input policy.
pub mod agent_policy;
mod command;
mod process;
mod protocol;

pub use command::{CodeMode, Command, HelpAnswer};
pub use process::{DesktopBackend, Host};
pub use protocol::{DesktopRequest, Event, Origin, PairDetails, Secret, parse_frame};

#[cfg(windows)]
mod windows_backend;
#[cfg(windows)]
pub use windows_backend::WindowsDesktopBackend;

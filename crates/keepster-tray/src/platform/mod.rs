//! The platform adapters: each draws the icon and its menu and shows notifications, and nothing else.
//! Every one is a [`Shell`]: it sets its toolkit up, then starts the connection with an [`Adapter`], and
//! runs whatever loop its toolkit needs on the main thread until the connection ends.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::run::{Action, Adapter};

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod capabilities;
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod handoff;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod headless;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The connection to the daemon, run on a thread of its own with what it draws and notifies through.
pub type Connection = Box<dyn FnOnce(Arc<dyn Adapter>) + Send>;

/// A platform's toolkit.
pub trait Shell {
    /// Set the toolkit up, start `connection` once it is — so that the first report the connection
    /// makes already says what the toolkit can do — and run the toolkit's loop on this thread until the
    /// connection ends.
    fn run(self: Box<Self>, connection: Connection);
}

/// This platform's shell, sending what is chosen from the menu to `actions`.
pub fn shell(actions: mpsc::UnboundedSender<Action>) -> Box<dyn Shell> {
    #[cfg(target_os = "linux")]
    return Box::new(linux::Linux::new(actions));
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacOs::new(actions));
    #[cfg(windows)]
    return Box::new(windows::Windows::new(actions));
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    return Box::new(headless::Headless::new(actions));
}

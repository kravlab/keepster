//! Keeping this process from ever downloading a cloud placeholder (ticket 75).
//!
//! A capture never opens a file whose metadata says its data is only in the cloud; that is the walk's
//! rule, in `keepster-store`. What the platform can add is set here, once, before any capture runs:
//! Windows is told to show this process placeholders as they are, so the walk sees the attributes that
//! mark one, and macOS is told not to materialise a dataless file this process touches, so a file freed
//! between the walk and the read fails with `EDEADLK` rather than download. Linux has neither notion:
//! a FUSE sync client fetches on read and marks nothing beforehand, so there is nothing to set.

use std::io;

#[cfg(target_os = "macos")]
mod macos;

/// Set what the platform offers against this process hydrating a cloud placeholder, for the whole
/// process and every thread it runs. A failure leaves the platform's default in place — on Windows,
/// placeholders exposed for most programs, and on macOS, the system's policy — and is the caller's to
/// report: the walk still recognises a placeholder from its metadata and never opens it.
pub fn guard_cloud_placeholders() -> io::Result<()> {
    #[cfg(windows)]
    {
        crate::windows::expose_placeholders()
    }

    #[cfg(target_os = "macos")]
    {
        macos::refuse_materialisation()
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Ok(())
    }
}

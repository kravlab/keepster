//! macOS: no dataless file is materialised by this process (ticket 75, TN3150).
#![allow(
    unsafe_code,
    reason = "setiopolicy_np has no safe wrapper in std or libc; it is confined to this module"
)]

use std::ffi::c_int;
use std::io;

// From <sys/resource.h>, which the `libc` crate does not carry.
const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: c_int = 3;
const IOPOL_SCOPE_PROCESS: c_int = 0;
const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: c_int = 1;

unsafe extern "C" {
    /// `setiopolicy_np(3)`, in libSystem since macOS 10.5.
    fn setiopolicy_np(iotype: c_int, scope: c_int, policy: c_int) -> c_int;
}

/// Turn materialisation of dataless files off for this process: touching one's data then fails with
/// `EDEADLK` instead of downloading it. Threads follow the process's policy unless they set their own,
/// and none here does.
pub(super) fn refuse_materialisation() -> io::Result<()> {
    // SAFETY: three integers by value; the call changes only this process's own I/O policy.
    let failed = unsafe {
        setiopolicy_np(
            IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
            IOPOL_SCOPE_PROCESS,
            IOPOL_MATERIALIZE_DATALESS_FILES_OFF,
        )
    };
    if failed != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

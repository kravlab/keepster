//! What only the Windows API can answer: the control channel's name and the access control that makes it
//! the owner's alone, a file's hard-link names, and where Git for Windows is installed (ticket 52).
//!
//! Every `unsafe` block in the workspace that touches the Windows API lives in this crate — here, or,
//! for which volume a path is on, in `volume/windows.rs` (ticket 56). Ticket 10 settled that shape when
//! it needed a link count that stable Rust does not expose. The crate-wide `unsafe_code = "deny"` lint
//! is lifted for these modules only, so each call is visible in one place and reviewable as a unit.
#![allow(
    unsafe_code,
    reason = "the Win32 calls below have no safe wrapper in std or tokio; they are confined to this module"
)]

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// The security identifier of the user this process runs as, in its string form.
///
/// The pipe is named after it rather than after the user name, because a SID is the identity Windows
/// actually checks: two accounts can share a display name, and a renamed account keeps its SID.
pub fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();

    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no closing, and `token` is a
    // valid out-pointer for the duration of the call.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedToken(token);

    let mut needed: u32 = 0;
    // SAFETY: asking for the required size with a null buffer is the documented first half of the
    // two-call pattern; it is expected to fail with ERROR_INSUFFICIENT_BUFFER.
    unsafe {
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }

    let mut buffer = vec![0u8; needed as usize];
    // SAFETY: `buffer` holds exactly the number of bytes the call just asked for.
    let read = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast::<c_void>(),
            needed,
            &mut needed,
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: on success the buffer starts with a `TOKEN_USER` whose `Sid` points inside it.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };

    let mut text = ptr::null_mut();
    // SAFETY: `sid` points into the buffer above, which outlives this call.
    let converted = unsafe { ConvertSidToStringSidW(sid, &mut text) };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    // Owned from here, so the allocation is released even if reading it unwinds.
    let text = LocalWideString(text);

    // SAFETY: on success the API hands back a NUL-terminated wide string allocated with LocalAlloc.
    Ok(unsafe { wide_to_string(text.0) })
}

/// A security descriptor granting access to one SID and nobody else.
///
/// The control channel is protected by OS permissions rather than by a secret (ticket 05), so this is
/// the whole boundary on Windows: the descriptor is protected (`P`), which stops inherited entries
/// from widening it, and grants generic-all to the owning SID alone.
pub struct OwnerOnlyAccess {
    descriptor: *mut c_void,
}

impl OwnerOnlyAccess {
    /// Build the descriptor for the given SID.
    pub fn for_sid(sid: &str) -> io::Result<Self> {
        let sddl = to_wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut descriptor = ptr::null_mut();

        // SAFETY: `sddl` is a NUL-terminated wide string that outlives the call, and `descriptor` is
        // a valid out-pointer.
        let built = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if built == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self { descriptor })
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }
}

impl Drop for OwnerOnlyAccess {
    fn drop(&mut self) {
        // SAFETY: freeing exactly what `ConvertStringSecurityDescriptorToSecurityDescriptorW`
        // allocated, once, at the end of this value's life.
        unsafe { LocalFree(self.descriptor.cast()) };
    }
}

// The descriptor is an owned allocation that is only read while creating a pipe instance, so it may
// travel with the listener that owns it.
// SAFETY: nothing else holds the pointer, and the Win32 calls that read it take no interior locks.
unsafe impl Send for OwnerOnlyAccess {}
// SAFETY: as above; the value is never mutated after construction.
unsafe impl Sync for OwnerOnlyAccess {}

/// Create one instance of the control pipe, reachable only by the owner.
///
/// `first_instance` must be true for the first instance and false for every later one: it is what
/// makes a second daemon fail to create the pipe instead of silently serving beside the first.
pub fn create_control_pipe(
    name: &OsStr,
    access: &OwnerOnlyAccess,
    first_instance: bool,
) -> io::Result<NamedPipeServer> {
    let mut attributes = access.attributes();

    // SAFETY: `attributes` lives across the call and points at a descriptor built above; the pointer
    // is only read during creation.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first_instance)
            .create_with_security_attributes_raw(
                name,
                ptr::addr_of_mut!(attributes).cast::<c_void>(),
            )
    }
}

/// A wide string the Win32 API allocated, freed when it goes out of scope.
struct LocalWideString(*mut u16);

impl Drop for LocalWideString {
    fn drop(&mut self) {
        // SAFETY: freeing exactly what `ConvertSidToStringSidW` allocated, once.
        unsafe { LocalFree(self.0.cast()) };
    }
}

/// A process token handle that closes itself.
struct OwnedToken(HANDLE);

impl Drop for OwnedToken {
    fn drop(&mut self) {
        // SAFETY: the handle came from `OpenProcessToken` and is closed exactly once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// # Safety
///
/// `text` must be a NUL-terminated wide string that stays valid for the length of the read.
unsafe fn wide_to_string(text: *mut u16) -> String {
    let mut length = 0;
    // SAFETY: the caller guarantees a NUL-terminated string, so the scan stops inside it.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` is the number of units before the terminator.
    let units = unsafe { std::slice::from_raw_parts(text, length) };

    String::from_utf16_lossy(units)
}

/// Every name of the file at `path`, as full paths — the hard links a Restore would break (ticket 10).
///
/// Windows has no link count in std's stable metadata and no `(dev, ino)` walk to find the other names;
/// `FindFirstFileNameW` lists them all, volume-relative, so each is put back on the file's volume.
pub fn hard_link_names(path: &std::path::Path) -> io::Result<Vec<std::path::PathBuf>> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Foundation::{
        ERROR_HANDLE_EOF, ERROR_MORE_DATA, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstFileNameW, FindNextFileNameW, GetVolumePathNameW,
    };

    /// Closes the name enumeration however the loop ends.
    struct Names(HANDLE);
    impl Drop for Names {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful `FindFirstFileNameW` and is closed once.
            unsafe {
                FindClose(self.0);
            }
        }
    }

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

    let mut volume = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `volume` holds the length passed.
    let found =
        unsafe { GetVolumePathNameW(wide.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) };
    if found == 0 {
        return Err(io::Error::last_os_error());
    }
    let end = volume
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(volume.len());
    let volume = std::path::PathBuf::from(OsString::from_wide(&volume[..end]));

    let name_of = |buffer: &[u16], length: u32| {
        // Up to the terminating NUL, which the reported length may or may not count.
        let filled = &buffer[..(length as usize).min(buffer.len())];
        let end = filled
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(filled.len());
        let units = &filled[..end];
        let name = OsString::from_wide(units);
        let relative = name.to_string_lossy().trim_start_matches('\\').to_owned();
        volume.join(relative)
    };

    let mut buffer = vec![0u16; 512];
    let mut length = buffer.len() as u32;
    let handle = loop {
        // SAFETY: `wide` is NUL-terminated, `length` is the capacity of `buffer` in UTF-16 units.
        let handle =
            unsafe { FindFirstFileNameW(wide.as_ptr(), 0, &mut length, buffer.as_mut_ptr()) };
        if handle != INVALID_HANDLE_VALUE {
            break Names(handle);
        }
        // SAFETY: reads the calling thread's last error, nothing else.
        if unsafe { GetLastError() } == ERROR_MORE_DATA {
            buffer.resize(length as usize, 0);
            continue;
        }
        return Err(io::Error::last_os_error());
    };

    let mut names = vec![name_of(&buffer, length)];
    loop {
        length = buffer.len() as u32;
        // SAFETY: the handle is open, and `length` is the capacity of `buffer` in UTF-16 units.
        let next = unsafe { FindNextFileNameW(handle.0, &mut length, buffer.as_mut_ptr()) };
        if next != 0 {
            names.push(name_of(&buffer, length));
            continue;
        }
        // SAFETY: reads the calling thread's last error, nothing else.
        match unsafe { GetLastError() } {
            ERROR_HANDLE_EOF => break,
            ERROR_MORE_DATA => buffer.resize(length as usize, 0),
            _ => return Err(io::Error::last_os_error()),
        }
    }

    Ok(names)
}

/// Where Git for Windows is installed, as `<InstallPath>\cmd\git.exe` — the one entry point its
/// maintainers support — from `HKCU\Software\GitForWindows\InstallPath` first, then `HKLM`, each in both
/// registry views (ticket 11). A per-user install writes `HKCU` only, which a service's environment does
/// not see but its registry does. Nothing is run to find it.
pub fn git_for_windows() -> Vec<std::path::PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY,
        RRF_SUBKEY_WOW6464KEY, RegGetValueW,
    };

    let wide = |text: &str| -> Vec<u16> { OsStr::new(text).encode_wide().chain(Some(0)).collect() };
    let subkey = wide("Software\\GitForWindows");
    let value = wide("InstallPath");

    let read = |hive: HKEY, view: u32| -> Option<std::path::PathBuf> {
        let flags = RRF_RT_REG_SZ | view;
        let mut bytes = 0u32;
        // SAFETY: both names are NUL-terminated; a null buffer asks only for the size, into `bytes`.
        let sized = unsafe {
            RegGetValueW(
                hive,
                subkey.as_ptr(),
                value.as_ptr(),
                flags,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut bytes,
            )
        };
        if sized != ERROR_SUCCESS || bytes == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        let mut length = (buffer.len() * 2) as u32;
        // SAFETY: as above, with `buffer` holding the `length` bytes passed.
        let read = unsafe {
            RegGetValueW(
                hive,
                subkey.as_ptr(),
                value.as_ptr(),
                flags,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut length,
            )
        };
        if read != ERROR_SUCCESS {
            return None;
        }
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        let install = std::path::PathBuf::from(OsString::from_wide(&buffer[..end]));

        (!install.as_os_str().is_empty()).then(|| install.join("cmd").join("git.exe"))
    };

    let mut found = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for view in [RRF_SUBKEY_WOW6464KEY, RRF_SUBKEY_WOW6432KEY] {
            if let Some(path) = read(hive, view) {
                if !found.contains(&path) {
                    found.push(path);
                }
            }
        }
    }

    found
}

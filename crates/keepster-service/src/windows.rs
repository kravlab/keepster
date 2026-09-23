//! The Windows control channel: its name, and the access control that makes it the owner's alone.
//!
//! Every `unsafe` block in the workspace that touches the Windows API is meant to live here — ticket
//! 10 already settled that shape when it needed a link count that stable Rust does not expose. The
//! crate-wide `unsafe_code = "deny"` lint is lifted for this module only, so each call is visible in
//! one place and reviewable as a unit.
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

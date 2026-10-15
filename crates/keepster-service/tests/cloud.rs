//! What the platform is told so that the daemon never hydrates a cloud placeholder (ticket 75), read
//! back from the platform itself after the guard has run.
#![cfg_attr(
    any(windows, target_os = "macos"),
    allow(
        unsafe_code,
        reason = "the platform's own query has no safe wrapper; the test reads the setting back through it"
    )
)]

use keepster_service::guard_cloud_placeholders;

#[cfg(target_os = "macos")]
#[test]
fn macos_materialises_no_dataless_file_for_this_process() {
    use std::ffi::c_int;

    // From <sys/resource.h>.
    const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: c_int = 3;
    const IOPOL_SCOPE_PROCESS: c_int = 0;
    const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: c_int = 1;
    unsafe extern "C" {
        fn getiopolicy_np(iotype: c_int, scope: c_int) -> c_int;
    }

    guard_cloud_placeholders().expect("the policy is set");

    // SAFETY: two integers by value; the call only reads this process's policy.
    let policy = unsafe {
        getiopolicy_np(
            IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
            IOPOL_SCOPE_PROCESS,
        )
    };
    assert_eq!(policy, IOPOL_MATERIALIZE_DATALESS_FILES_OFF);
}

#[cfg(windows)]
#[test]
fn windows_shows_this_process_placeholders_as_they_are() {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    // From <ntifs.h>.
    const PHCM_EXPOSE_PLACEHOLDERS: i8 = 2;

    guard_cloud_placeholders().expect("the mode is set");

    let ntdll: Vec<u16> = "ntdll.dll\0".encode_utf16().collect();
    // SAFETY: `ntdll` is NUL-terminated, and ntdll is mapped into every process.
    let module = unsafe { GetModuleHandleW(ntdll.as_ptr()) };
    // SAFETY: `module` is ntdll's handle and the name is a NUL-terminated ANSI string.
    let found = unsafe {
        GetProcAddress(
            module,
            c"RtlQueryProcessPlaceholderCompatibilityMode"
                .as_ptr()
                .cast(),
        )
    }
    .expect("the runners' Windows has the query, which arrived with the setting in 1803");
    // SAFETY: `ntifs.h` declares the export as `CHAR RtlQueryProcessPlaceholderCompatibilityMode(VOID)`.
    let query = unsafe {
        std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "system" fn() -> i8>(
            found,
        )
    };
    // SAFETY: no arguments; the call only reads this process's setting.
    assert_eq!(unsafe { query() }, PHCM_EXPOSE_PLACEHOLDERS);
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn linux_has_nothing_to_set_and_says_so_without_failing() {
    // No placeholder exists here to be guarded against (ticket 75): the guard succeeds doing nothing.
    assert!(guard_cloud_placeholders().is_ok());
}

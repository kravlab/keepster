//! The Start-menu shortcut `keepster install` writes on Windows (ticket 68), read back through the shell's
//! own interfaces: a toast from an unpackaged program shows only while a shortcut carries its
//! AppUserModelID. The shortcut is written into a temporary directory; the user's `Programs` folder is a
//! change to the machine, which no test makes. Whether Windows then shows a toast is not observed here.
#![cfg(windows)]
#![allow(
    unsafe_code,
    reason = "the shell's COM interfaces, used to read the shortcut back, have no safe wrapper"
)]

use std::path::Path;

use keepster_service::create_shortcut;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{HSTRING, Interface};

/// The AppUserModelID and the target the shortcut at `link` carries.
fn read_back(link: &Path) -> (String, String) {
    // SAFETY: COM is initialised on this thread for the calls below and uninitialised after them; every
    // interface is released before that, when it goes out of scope inside the block.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .expect("COM starts");
        let read = {
            let shell_link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).expect("a shell link");
            shell_link
                .cast::<IPersistFile>()
                .expect("a file")
                .Load(&HSTRING::from(link.as_os_str()), STGM_READ)
                .expect("the shortcut loads");
            let app_id = shell_link
                .cast::<IPropertyStore>()
                .expect("its properties")
                .GetValue(&PKEY_AppUserModel_ID)
                .expect("the AppUserModelID")
                .to_string();
            let mut target = [0_u16; 1024];
            shell_link
                .GetPath(&mut target, std::ptr::null_mut(), 0)
                .expect("its target");
            let end = target
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(target.len());
            (app_id, String::from_utf16_lossy(&target[..end]))
        };
        CoUninitialize();
        read
    }
}

#[test]
fn the_shortcut_starts_the_tray_and_carries_its_app_user_model_id() {
    let directory = tempfile::tempdir().expect("a directory");
    let tray = directory.path().join("keepster-tray.exe");
    std::fs::write(&tray, b"").expect("a tray to point at");
    let link = directory.path().join("Programs").join("Keepster.lnk");

    create_shortcut(&link, &tray).expect("the shortcut is written");

    let (app_id, target) = read_back(&link);
    assert_eq!(
        app_id, "Keepster.Tray",
        "the identity the tray shows toasts under"
    );
    assert_eq!(Path::new(&target), tray);
}

#[test]
fn writing_the_shortcut_again_replaces_it() {
    let directory = tempfile::tempdir().expect("a directory");
    let link = directory.path().join("Keepster.lnk");
    let old = directory.path().join("old.exe");
    let new = directory.path().join("new.exe");
    for tray in [&old, &new] {
        std::fs::write(tray, b"").expect("a tray to point at");
    }

    create_shortcut(&link, &old).expect("the first shortcut");
    create_shortcut(&link, &new).expect("the second shortcut");

    assert_eq!(Path::new(&read_back(&link).1), new);
}

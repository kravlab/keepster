//! Volume identity as the gate of capture (ticket 56, ADR 0011): what a Root's recorded place on its
//! volume says about the tree in front of the daemon, where a remounted volume puts the Root, and where
//! the platform's watcher is documented blind.

use std::path::{Path, PathBuf};

use keepster_core::{Platform, VolumePlace, VolumeVerdict, verify_volume, watch_blind};

fn place(identity: &str, within: &str) -> VolumePlace {
    VolumePlace {
        identity: identity.to_owned(),
        within: within.to_owned(),
    }
}

#[test]
fn the_same_volume_at_the_same_place_is_ours() {
    let recorded = place("uuid:1234", "projects/app");

    assert_eq!(
        verify_volume(Some(&recorded), Some(&place("uuid:1234", "projects/app"))),
        VolumeVerdict::Ours
    );
}

#[test]
fn another_volume_or_none_at_all_is_not_ours() {
    // An unmounted mount point is an ordinary directory on the volume beneath it: present, empty, and
    // not the tree that was registered.
    let recorded = place("uuid:1234", "app");

    for found in [
        Some(place("uuid:root", "media/disk/app")),
        Some(place("uuid:1234", "elsewhere/app")),
        None,
    ] {
        assert_eq!(
            verify_volume(Some(&recorded), found.as_ref()),
            VolumeVerdict::Elsewhere,
            "{found:?}"
        );
    }
}

#[test]
fn a_root_whose_volume_could_not_be_identified_is_unproven_whatever_is_found_now() {
    // Nothing recorded means nothing to compare: capture proceeds, guarded against total deletion.
    for found in [Some(place("uuid:1234", "app")), None] {
        assert_eq!(
            verify_volume(None, found.as_ref()),
            VolumeVerdict::Unproven,
            "{found:?}"
        );
    }
}

#[test]
fn a_volume_mounted_elsewhere_puts_the_root_at_the_same_place_within_it() {
    let recorded = place("uuid:1234", "projects/app");

    assert_eq!(
        recorded.at(Path::new("/run/media/someone/DISK1")),
        PathBuf::from("/run/media/someone/DISK1")
            .join("projects")
            .join("app")
    );
    assert_eq!(
        place("uuid:1234", "").at(Path::new("/media/disk")),
        PathBuf::from("/media/disk"),
        "a Root that is the volume itself"
    );
}

#[test]
fn the_watcher_is_blind_exactly_where_the_platform_documents_it() {
    // Linux: inotify and fanotify never see a remote change, and FUSE emits no fsnotify event for one.
    for blind in [
        "nfs",
        "nfs4",
        "cifs",
        "smb3",
        "9p",
        "ceph",
        "fuse.sshfs",
        "fuse",
    ] {
        assert!(watch_blind(Platform::Linux, blind, false), "{blind}");
    }
    for local in ["ext4", "btrfs", "xfs", "tmpfs", "overlay", "fuseblk"] {
        assert!(!watch_blind(Platform::Linux, local, false), "{local}");
    }
    assert!(
        watch_blind(Platform::Linux, "something_new", true),
        "a mount the platform says is remote"
    );

    // macOS: FSEvents sees only local writes.
    assert!(watch_blind(Platform::MacOs, "smbfs", true));
    assert!(!watch_blind(Platform::MacOs, "apfs", false));

    // Windows: ReadDirectoryChangesW documents SMB as supported.
    assert!(!watch_blind(Platform::Windows, "", true));
}

//! The external `git` as pure decisions (ticket 52, ticket 11): its version read leniently, where it is
//! looked for on each platform, and what a person is told to run when it is missing.

use std::path::{Path, PathBuf};

use keepster_core::{GIT_MINIMUM, GitVersion, PackageFamily, Platform, git_candidates};

fn version(output: &str) -> Option<GitVersion> {
    GitVersion::parse(output)
}

#[test]
fn a_version_is_read_from_its_first_three_numbers_and_nothing_else() {
    assert_eq!(
        version("git version 2.43.0\n"),
        Some(GitVersion::new(2, 43, Some(0)))
    );
    assert_eq!(
        version("git version 2.45.1.windows.1"),
        Some(GitVersion::new(2, 45, Some(1))),
        "Git for Windows appends its own"
    );
    assert_eq!(
        version("git version 2.39.5 (Apple Git-154)"),
        Some(GitVersion::new(2, 39, Some(5))),
        "Apple's parenthesised tail, with a dot of its own, is never read"
    );
    assert_eq!(
        version("git version 2.56.0.rc0.123.g1a2b3c4.dirty"),
        Some(GitVersion::new(2, 56, Some(0)))
    );
    assert_eq!(
        version("git version 2.51"),
        Some(GitVersion::new(2, 51, None)),
        "a missing third number is unknown, not an error"
    );
    assert_eq!(
        version("git version 2.50.rc1"),
        Some(GitVersion::new(2, 50, None))
    );
    for foreign in [
        "",
        "version 2.43.0",
        "git 2.43.0",
        "git version two",
        "hub version 2.14.2",
    ] {
        assert_eq!(version(foreign), None, "{foreign}");
    }
}

#[test]
fn the_minimum_is_2_32_and_an_unknown_third_number_is_given_the_benefit() {
    assert_eq!(GIT_MINIMUM, GitVersion::new(2, 32, Some(0)));
    assert!(GitVersion::new(2, 32, Some(0)).meets(GIT_MINIMUM));
    assert!(GitVersion::new(2, 32, None).meets(GIT_MINIMUM));
    assert!(GitVersion::new(3, 0, Some(0)).meets(GIT_MINIMUM));
    assert!(!GitVersion::new(2, 31, Some(9)).meets(GIT_MINIMUM));
    assert!(!GitVersion::new(1, 99, None).meets(GIT_MINIMUM));
    assert_eq!(GitVersion::new(2, 31, None).to_string(), "2.31");
    assert_eq!(GIT_MINIMUM.to_string(), "2.32.0");
}

#[test]
fn linux_looks_in_stable_places_and_never_in_a_nix_store_hash() {
    let found = git_candidates(Platform::Linux, Some(Path::new("/home/me")), None);

    assert_eq!(found.first(), Some(&PathBuf::from("/usr/bin/git")));
    for expected in [
        "/usr/local/bin/git",
        "/run/current-system/sw/bin/git",
        "/home/me/.nix-profile/bin/git",
        "/home/me/.guix-profile/bin/git",
        "/home/linuxbrew/.linuxbrew/bin/git",
        "/snap/bin/git",
    ] {
        assert!(
            found.contains(&PathBuf::from(expected)),
            "{expected}: {found:?}"
        );
    }
    assert!(found.iter().all(|path| !path.starts_with("/nix/store")));
}

#[test]
fn macos_never_looks_at_the_usr_bin_stub() {
    let found = git_candidates(
        Platform::MacOs,
        Some(Path::new("/Users/me")),
        Some(Path::new("/Applications/Xcode.app/Contents/Developer")),
    );

    assert_eq!(
        found,
        [
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
            "/opt/local/bin/git",
            "/Library/Developer/CommandLineTools/usr/bin/git",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
        ]
        .map(PathBuf::from)
    );
}

#[test]
fn windows_takes_its_candidates_from_the_registry_alone() {
    assert!(git_candidates(Platform::Windows, Some(Path::new("C:/Users/me")), None).is_empty());
}

#[test]
fn a_missing_git_is_answered_with_the_platform_s_own_command() {
    let hint = |family: PackageFamily| family.install_command();

    assert_eq!(hint(PackageFamily::Apt), Some("sudo apt install git"));
    assert_eq!(hint(PackageFamily::Dnf), Some("sudo dnf install git"));
    assert_eq!(hint(PackageFamily::Pacman), Some("sudo pacman -S git"));
    assert_eq!(hint(PackageFamily::Zypper), Some("sudo zypper install git"));
    assert_eq!(hint(PackageFamily::Apk), Some("sudo apk add git"));
    assert_eq!(hint(PackageFamily::Homebrew), Some("brew install git"));
    assert_eq!(
        hint(PackageFamily::XcodeSelect),
        Some("xcode-select --install")
    );
    assert_eq!(
        hint(PackageFamily::Winget),
        Some("winget install --id Git.Git -e")
    );
    assert_eq!(hint(PackageFamily::Unknown), None);
    assert!(
        PackageFamily::Winget
            .install_note()
            .is_some_and(|note| note.contains("elevation"))
    );
    assert!(
        PackageFamily::Unknown
            .install_note()
            .is_some_and(|note| note.contains("git.path"))
    );
}

#[test]
fn a_linux_family_is_read_from_os_release() {
    let family = PackageFamily::from_os_release;

    assert_eq!(family("ID=ubuntu\nID_LIKE=debian\n"), PackageFamily::Apt);
    assert_eq!(
        family("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"),
        PackageFamily::Dnf
    );
    assert_eq!(family("ID=fedora\n"), PackageFamily::Dnf);
    assert_eq!(family("ID=arch\n"), PackageFamily::Pacman);
    assert_eq!(
        family("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
        PackageFamily::Zypper
    );
    assert_eq!(family("ID=alpine\n"), PackageFamily::Apk);
    assert_eq!(
        family("ID=nixos\n"),
        PackageFamily::Unknown,
        "Nix is configured, not installed into"
    );
    assert_eq!(family(""), PackageFamily::Unknown);
}

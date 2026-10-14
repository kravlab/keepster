//! What the tripwire records about the machine beside every number it prints (ticket 30), described
//! from `/proc` by pure functions tested here on texts written to the kernel's documented formats.

use std::path::{Path, PathBuf};

use keepster_service::mountinfo::makedev;
use keepster_testkit::environment::{
    Cpu, Environment, EnvironmentError, Filesystem, Readings, cpu, describe, kernel, memory,
};

/// A mount table: the root filesystem, a data volume with a scratch disk beneath it, and a tmpfs.
const MOUNTINFO: &[u8] = b"\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
23 22 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:12 - proc proc rw
24 22 253:0 / /data rw,noatime shared:2 master:7 - btrfs /dev/mapper/example rw,ssd,subvol=/
25 22 0:30 / /tmp rw,nosuid,nodev shared:3 - tmpfs tmpfs rw,size=1024k
26 24 8:17 / /data/scratch rw - xfs /dev/sdb1 rw
";

#[test]
fn only_memory_backed_filesystems_are_in_memory() {
    for (kind, in_memory) in [
        ("tmpfs", true),
        ("ramfs", true),
        ("ext4", false),
        ("btrfs", false),
        ("xfs", false),
    ] {
        let filesystem = Filesystem {
            kind: kind.to_owned(),
            source: "somewhere".to_owned(),
            mount_point: PathBuf::from("/"),
        };
        assert_eq!(filesystem.in_memory(), in_memory, "{kind}");
    }
}

#[test]
fn the_cpu_is_its_first_model_name_and_its_count_of_processors() {
    let cpuinfo = "\
processor\t: 0
vendor_id\t: ExampleVendor
model name\t: Example CPU 9000 @ 3.00GHz
flags\t\t: fpu vme

processor\t: 1
vendor_id\t: ExampleVendor
model name\t: Example CPU 9000 @ 3.00GHz (second)
flags\t\t: fpu vme
";

    assert_eq!(
        cpu(cpuinfo),
        Cpu {
            model: Some("Example CPU 9000 @ 3.00GHz".to_owned()),
            threads: 2,
        }
    );
}

#[test]
fn a_cpu_that_names_no_model_is_counted_all_the_same() {
    // Some architectures' `/proc/cpuinfo` has no `model name` line.
    let cpuinfo = "\
processor\t: 0
BogoMIPS\t: 50.00
CPU implementer\t: 0x41

processor\t: 1
BogoMIPS\t: 50.00
CPU implementer\t: 0x41
";

    assert_eq!(
        cpu(cpuinfo),
        Cpu {
            model: None,
            threads: 2,
        }
    );
    assert_eq!(
        cpu(""),
        Cpu {
            model: None,
            threads: 0,
        }
    );
}

#[test]
fn the_memory_is_the_total_in_bytes() {
    let meminfo = "\
MemTotal:       16303456 kB
MemFree:         1203948 kB
MemAvailable:    9876543 kB
";

    assert_eq!(memory(meminfo), Some(16_303_456 * 1024));
}

#[test]
fn a_memory_table_without_a_total_it_can_hold_says_nothing() {
    assert_eq!(memory("MemFree:         1203948 kB\n"), None);
    assert_eq!(memory("MemTotal:       lots kB\n"), None);
    assert_eq!(memory("MemTotal:       16303456 MB\n"), None);
    assert_eq!(memory(""), None);
    // As many kibibytes as a u64 holds is more bytes than one does.
    assert_eq!(memory("MemTotal:       18446744073709551615 kB\n"), None);
}

#[test]
fn the_kernel_is_its_release_string() {
    assert_eq!(
        kernel("6.1.0-example-amd64\n"),
        Some("6.1.0-example-amd64".to_owned())
    );
    assert_eq!(kernel("  \n"), None);
}

#[test]
fn a_machine_is_described_from_what_was_read() {
    assert_eq!(
        describe(&readings()).expect("a measurable machine"),
        Environment {
            cpu: Cpu {
                model: Some("Example CPU".to_owned()),
                threads: 4,
            },
            available_threads: 2,
            memory_bytes: 2048,
            kernel: "6.1.0-example".to_owned(),
            filesystem: Filesystem {
                kind: "btrfs".to_owned(),
                source: "/dev/mapper/example".to_owned(),
                mount_point: PathBuf::from("/data"),
            },
        }
    );
}

#[test]
fn the_filesystem_is_the_one_of_the_fixture_directorys_own_device() {
    let scratch = Readings {
        directory: Path::new("/data/scratch/bench"),
        device: makedev(8, 17),
        ..readings()
    };

    assert_eq!(
        describe(&scratch)
            .expect("a measurable machine")
            .filesystem
            .kind,
        "xfs"
    );
}

#[test]
fn a_fixture_directory_in_memory_is_refused_naming_the_filesystem() {
    for (directory, device, mount_point) in [
        ("/tmp/bench", makedev(0, 30), "/tmp"),
        // A tmpfs mounted over `/data` after `/data/scratch` hides it: the device tells.
        ("/data/scratch/bench", makedev(0, 40), "/data"),
    ] {
        let mountinfo = [MOUNTINFO, b"40 24 0:40 / /data rw - tmpfs tmpfs rw\n"].concat();
        let refused = describe(&Readings {
            directory: Path::new(directory),
            device,
            mountinfo: &mountinfo,
            ..readings()
        })
        .expect_err(directory);

        assert!(
            matches!(
                &refused,
                EnvironmentError::InMemory { kind, mount_point: point }
                    if kind == "tmpfs" && point == Path::new(mount_point)
            ),
            "{directory}: {refused:?}"
        );
        let message = refused.to_string();
        assert!(
            message.contains("tmpfs") && message.contains(mount_point),
            "{message}"
        );
    }
}

#[test]
fn a_fixture_directory_no_mount_holds_is_refused_naming_it() {
    let refused = describe(&Readings {
        mountinfo: b"23 22 0:21 / /proc rw - proc proc rw\n",
        ..readings()
    })
    .expect_err("no mount holds it");

    assert!(
        matches!(&refused, EnvironmentError::NoMount { directory } if directory == Path::new("/data/bench")),
        "{refused:?}"
    );
    assert!(refused.to_string().contains("/data/bench"), "{refused}");
}

#[test]
fn a_proc_file_that_does_not_say_what_it_should_is_refused_naming_it() {
    let cases = [
        (
            Readings {
                cpuinfo: "model name\t: Example CPU\n",
                ..readings()
            },
            "/proc/cpuinfo",
        ),
        (
            Readings {
                meminfo: "MemFree: 1 kB\n",
                ..readings()
            },
            "/proc/meminfo",
        ),
        (
            Readings {
                osrelease: "\n",
                ..readings()
            },
            "/proc/sys/kernel/osrelease",
        ),
    ];
    for (texts, file) in cases {
        let refused = describe(&texts).expect_err(file);
        assert!(
            matches!(&refused, EnvironmentError::Malformed { path, .. } if path == Path::new(file)),
            "{file}: {refused:?}"
        );
    }
}

#[test]
#[should_panic(expected = "from the root")]
fn a_relative_fixture_directory_is_a_mistake() {
    let _ = describe(&Readings {
        directory: Path::new("data/bench"),
        ..readings()
    });
}

/// What a run on a measurable machine reads: four processors of which two are this process's, and a
/// fixture directory on the btrfs volume.
fn readings() -> Readings<'static> {
    Readings {
        directory: Path::new("/data/bench"),
        device: makedev(253, 0),
        available_threads: 2,
        mountinfo: MOUNTINFO,
        cpuinfo: "\
processor\t: 0
model name\t: Example CPU
processor\t: 1
processor\t: 2
processor\t: 3
",
        meminfo: "MemTotal:       2 kB\n",
        osrelease: "6.1.0-example\n",
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use keepster_testkit::environment::{EnvironmentError, detect};

    #[test]
    fn this_machine_is_described_or_refused_for_holding_the_fixture_in_memory() {
        // Where the tripwire puts its fixture by default; `/tmp` is tmpfs on the reference machine.
        let directory =
            tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("a temporary directory");

        match detect(directory.path()) {
            Ok(environment) => {
                assert!(environment.cpu.threads > 0, "{environment:?}");
                assert!(environment.available_threads > 0, "{environment:?}");
                assert!(environment.memory_bytes > 0, "{environment:?}");
                assert!(!environment.kernel.is_empty(), "{environment:?}");
                assert!(!environment.filesystem.kind.is_empty(), "{environment:?}");
                assert!(!environment.filesystem.in_memory(), "{environment:?}");
                assert!(
                    directory
                        .path()
                        .canonicalize()
                        .expect("a real directory")
                        .starts_with(&environment.filesystem.mount_point),
                    "{environment:?}"
                );
            }
            Err(EnvironmentError::InMemory { kind, .. }) => {
                assert!(kind == "tmpfs" || kind == "ramfs", "{kind}");
            }
            Err(other) => panic!("{other}"),
        }
    }

    #[test]
    fn a_directory_that_is_not_there_is_refused() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let missing = directory.path().join("missing");

        assert!(matches!(
            detect(&missing),
            Err(EnvironmentError::Unreadable { path, .. }) if path == missing
        ));
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn every_other_platform_is_refused() {
    // The tripwire has one reference, a Linux machine (ticket 30).
    let directory = tempfile::tempdir().expect("a temporary directory");

    assert!(matches!(
        keepster_testkit::environment::detect(directory.path()),
        Err(EnvironmentError::NotLinux)
    ));
}

#[test]
fn the_machines_own_git_is_found_at_a_stable_place() {
    // Found as the daemon finds it, so the tripwire maintains its stores with the `git` a user's would.
    let git = keepster_testkit::environment::git().expect("a git on the machine running the tests");

    assert!(git.path().is_absolute(), "{}", git.path().display());
    assert!(git.path().is_file(), "{}", git.path().display());
}

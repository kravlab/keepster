//! The Maintenance Pass against a real store and a real `git` (ticket 52): deleting a ref frees nothing,
//! the pass frees what Retention condemned, every invocation is hermetic, and a packed store reads back
//! as it did loose.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, Ladder, Newest, RootId, RootPath, Trigger};
#[cfg(not(windows))]
use keepster_core::{Platform, git_candidates};
use keepster_store::{Depth, Git, GitUnavailable, HistoryStore};

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;
const DAY: u64 = 24 * 3600;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The machine's own `git`, found as the daemon finds it.
fn git() -> Git {
    #[cfg(windows)]
    let candidates = keepster_service::git_for_windows();
    #[cfg(not(windows))]
    let candidates = git_candidates(
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        },
        std::env::home_dir().as_deref(),
        None,
    );

    Git::locate(None, &candidates, None).expect("a git on the machine running the tests")
}

/// Every byte under `directory`.
fn size(directory: &Path) -> u64 {
    fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| match entry.file_type() {
                    Ok(kind) if kind.is_dir() => size(&entry.path()),
                    _ => entry.metadata().map_or(0, |metadata| metadata.len()),
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Bytes no compressor shrinks, different for every `seed`.
fn noise(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    (0..length)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 56) as u8
        })
        .collect()
}

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    store_path: PathBuf,
    store: HistoryStore,
    id: RootId,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(&root).expect("the Root");
        let store_path = directory.path().join("history.git");
        let store = HistoryStore::open(&store_path, directory.path().join("index.sqlite"))
            .expect("a store");

        Self {
            _directory: directory,
            root,
            store_path,
            store,
            id: RootId::generate(at(NOW)),
        }
    }

    fn capture(&self, moment: u64) -> keepster_core::SnapshotId {
        self.store
            .capture(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(moment),
            )
            .expect("a Snapshot")
            .id
    }

    fn objects(&self) -> u64 {
        size(&self.store_path.join("objects"))
    }
}

#[test]
fn the_pass_reclaims_what_retention_condemned_and_a_deleted_ref_alone_frees_nothing() {
    let fixture = Fixture::new();
    // Forty days ago, a megabyte at a time, each soon replaced; then today, something small.
    for day in 0..4 {
        fs::write(fixture.root.join("blob.bin"), noise(day, 1 << 20)).expect("a large file");
        fixture.capture(NOW - 40 * DAY + day * 3600);
    }
    fs::write(fixture.root.join("blob.bin"), b"small now").expect("a small file");
    fixture.capture(NOW - 60);
    let before = fixture.objects();

    let retained = fixture
        .store
        .retain(&Ladder::default(), at(NOW), Newest::Survives)
        .expect("a pass");
    assert!(retained.deleted_segments > 0, "{retained:?}");
    assert_eq!(
        fixture.objects(),
        before,
        "a deleted ref only makes its objects unreachable: this is why the pass exists"
    );

    let maintained = fixture.store.maintain(&git()).expect("maintained");

    let after = fixture.objects();
    assert!(
        after < before / 4,
        "the condemned megabytes are gone: {before} bytes before, {after} after"
    );
    assert_eq!(
        maintained.reclaimed_bytes,
        before - after,
        "and the pass says how much"
    );
}

#[test]
fn a_packed_store_reads_back_as_it_did_loose() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.root.join("src")).expect("a directory");
    fs::write(fixture.root.join("src/lib.rs"), b"pub fn kept() {}\n").expect("a file");
    fs::write(fixture.root.join("large.bin"), noise(7, 3 << 20)).expect("a large file");
    let mut text = noise(9, 1 << 20);
    text.iter_mut().for_each(|byte| *byte = b'a' + (*byte % 26));
    fs::write(fixture.root.join("large.txt"), &text).expect("a large text");
    let snapshot = fixture.capture(NOW - 60);
    let loose = fixture
        .store
        .entries(snapshot, &RootPath::root(), Depth::Infinity)
        .expect("a listing");

    fixture.store.maintain(&git()).expect("maintained");
    assert!(
        fs::read_dir(fixture.store_path.join("objects/pack"))
            .expect("packs")
            .count()
            > 0,
        "the pass packed the store"
    );

    let packed = fixture
        .store
        .entries(snapshot, &RootPath::root(), Depth::Infinity)
        .expect("a listing");
    for (was, is) in loose.entries.iter().zip(&packed.entries) {
        assert_eq!(
            (&was.path, was.kind, was.size),
            (&is.path, is.kind, is.size)
        );
        assert!(
            is.binary.is_none() || is.binary == was.binary,
            "a packed file's binary flag is either probed as before or not probed at all: {is:?}"
        );
    }
    let binary = |path: &str| {
        packed
            .entries
            .iter()
            .find(|entry| entry.path.to_lossy() == path)
            .map(|entry| entry.binary)
            .expect("listed")
    };
    assert_eq!(
        binary("src/lib.rs"),
        Some(false),
        "a small packed file is still probed"
    );
    assert_eq!(
        binary("large.txt"),
        Some(false),
        "up to a megabyte is read to probe it"
    );
    assert_eq!(
        binary("large.bin"),
        None,
        "a larger packed file would be inflated whole to read its first 8000 bytes, so it is not probed"
    );

    let content = fixture
        .store
        .content(snapshot, &RootPath::parse("large.txt").expect("a path"))
        .expect("the version");
    let mut range = Vec::new();
    content
        .read(100_000, 16, &mut |chunk| {
            range.extend_from_slice(chunk);
            Ok(())
        })
        .expect("a range");
    assert_eq!(range, &text[100_000..100_016]);
}

#[test]
fn a_missing_or_unusable_git_is_said_to_be_so_and_never_guessed_at() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let nowhere = directory.path().join("no-git-here");

    assert_eq!(
        Git::locate(None, std::slice::from_ref(&nowhere), None),
        Err(GitUnavailable::Missing {
            looked: vec![nowhere.clone()]
        })
    );
    let stub = directory.path().join("stub");
    fs::write(&stub, b"").expect("a stub");
    assert_eq!(
        Git::locate(None, std::slice::from_ref(&nowhere), Some(&stub)),
        Err(GitUnavailable::MacosStub)
    );
    let configured = Git::locate(Some(&nowhere), &[git().path().to_path_buf()], None)
        .expect_err("a configured path is authoritative");
    assert_eq!(configured.reason(), "not_executable", "{configured}");
}

#[cfg(unix)]
mod fake {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A `git` that is a shell script: says `version`, and writes its environment and arguments to
    /// `record` before doing nothing.
    fn fake_git(directory: &Path, version: &str, record: &Path) -> PathBuf {
        let path = directory.join("git");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'git version {version}'; exit 0; fi\n\
                 env > '{record}'\necho -- \"$@\" >> '{record}'\n",
                record = record.display()
            ),
        )
        .expect("a script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        // Another test forking while the script was open for writing holds it open until it execs, and
        // the kernel refuses to run a file open for writing (ETXTBSY): wait that moment out.
        for _ in 0..200 {
            match std::process::Command::new(&path).arg("--version").output() {
                Err(error) if error.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                _ => break,
            }
        }
        path
    }

    #[test]
    fn too_old_a_git_is_refused_by_its_version() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let record = directory.path().join("record");
        let old = fake_git(directory.path(), "2.31.8", &record);

        let refused = Git::locate(None, &[old], None).expect_err("too old");
        assert_eq!(refused.reason(), "too_old", "{refused}");
    }

    #[test]
    fn every_invocation_is_hermetic() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let record = directory.path().join("record");
        let script = fake_git(directory.path(), "2.43.0 (Apple Git-115)", &record);
        let git = Git::probe(&script).expect("new enough");
        let fixture = Fixture::new();
        fixture.capture(NOW);

        fixture.store.maintain(&git).expect("maintained");

        let recorded = fs::read_to_string(&record).expect("what the script saw");
        let mut variables: Vec<&str> = recorded
            .lines()
            .filter(|line| !line.starts_with("--"))
            .map(|line| line.split('=').next().unwrap_or_default())
            .filter(|name| !["PWD", "SHLVL", "_", "OLDPWD"].contains(name))
            .collect();
        variables.sort_unstable();
        variables.retain(|name| !["TMPDIR", "TEMP", "TMP"].contains(name));
        assert_eq!(
            variables,
            ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "LC_ALL"],
            "nothing of the daemon's environment: {recorded}"
        );
        assert!(recorded.contains("GIT_CONFIG_NOSYSTEM=1"), "{recorded}");
        let global = recorded
            .lines()
            .find_map(|line| line.strip_prefix("GIT_CONFIG_GLOBAL="))
            .expect("a global configuration");
        assert_eq!(fs::read(global).expect("it exists"), b"", "and it is empty");

        let arguments = recorded
            .lines()
            .find_map(|line| line.strip_prefix("-- "))
            .expect("the arguments");
        assert_eq!(
            arguments,
            format!(
                "--git-dir={} -c core.logAllRefUpdates=false -c core.filemode=false gc --prune=now --quiet",
                fixture.store_path.display()
            ),
            "the store named, its correctness keys inline, and never --force"
        );
    }
}

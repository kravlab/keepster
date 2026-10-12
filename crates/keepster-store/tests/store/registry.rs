//! The Registry's one hard promise: a reader sees either the previous record or the new one.
//!
//! Asserted against real files, because the promise is made by the filesystem — a temporary file, a
//! flush, a rename — and a fake would prove nothing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use keepster_core::RootId;
use keepster_store::{Registry, RegistryError, RootRecord, Standing};

#[test]
fn a_record_being_rewritten_is_never_read_half_written() {
    let base = tempfile::tempdir().expect("a temporary store base");
    let registry = Registry::open(base.path()).expect("an empty registry");

    let id = RootId::generate(SystemTime::UNIX_EPOCH);
    registry
        .insert(&RootRecord {
            id,
            path: PathBuf::from("/start"),
            volume: None,
        })
        .expect("the first record");

    let stop = AtomicBool::new(false);

    std::thread::scope(|scope| {
        // One thread rewrites the record — the shape a move takes — while the other reads it.
        scope.spawn(|| {
            for round in 0..300 {
                registry
                    .insert(&RootRecord {
                        id,
                        path: PathBuf::from(format!("/moved/{round}")),
                        volume: None,
                    })
                    .expect("a rewrite");
            }

            stop.store(true, Ordering::Release);
        });

        let mut reads = 0_u32;
        while !stop.load(Ordering::Acquire) {
            let entries = registry
                .list()
                .expect("a reader never sees a half-written record");

            assert_eq!(entries.len(), 1, "a rewrite must not double the record");
            assert_eq!(entries[0].id, id, "the identity survives every rewrite");
            entries[0]
                .record
                .as_ref()
                .expect("every read parses, or the write was not atomic");
            reads += 1;
        }

        assert!(reads > 0, "the reader must actually have read something");
    });

    let leftovers: Vec<_> = std::fs::read_dir(registry.root_dir(id))
        .expect("the Root's directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "root.toml")
        .collect();

    assert!(
        leftovers.is_empty(),
        "a completed write leaves no temporary file behind, found {leftovers:?}"
    );
}

#[test]
fn a_record_from_an_older_container_is_rewritten_in_this_one_and_read_as_it_was() {
    // ADR 0010: the container version is the only one rewritten automatically — small, atomic and not
    // history. Every one older than this build's is read and then written back in its own.
    let base = tempfile::tempdir().expect("a temporary store base");
    let registry = Registry::open(base.path()).expect("an empty registry");
    let id = RootId::generate(SystemTime::UNIX_EPOCH);
    let dir = registry.root_dir(id);
    std::fs::create_dir_all(&dir).expect("its directory");
    let record = dir.join("root.toml");
    std::fs::write(
        &record,
        format!("version = 0\nid = \"{id}\"\npath = \"/project\"\n"),
    )
    .expect("an older record");

    let entries = registry.list().expect("a listing");
    assert_eq!(
        entries[0].record.as_ref().expect("readable"),
        &RootRecord {
            id,
            path: PathBuf::from("/project"),
            volume: None,
        }
    );
    assert!(
        std::fs::read_to_string(&record)
            .expect("there")
            .contains("version = 0"),
        "reading writes nothing"
    );

    assert_eq!(registry.upgrade().expect("upgraded"), 1);

    assert_eq!(
        registry.list().expect("a listing")[0]
            .record
            .as_ref()
            .expect("readable"),
        entries[0].record.as_ref().expect("readable"),
        "it describes what it did"
    );
    assert_eq!(registry.upgrade().expect("upgraded"), 0, "once");
    let rewritten = std::fs::read_to_string(&record).expect("still there");
    assert!(
        rewritten.contains(&format!("version = {}", keepster_store::CONTAINER_VERSION)),
        "{rewritten}"
    );
    assert!(
        !dir.join("root.toml.new").exists(),
        "replaced atomically, nothing left beside it"
    );
}

#[test]
fn a_root_s_place_on_its_volume_is_recorded_and_survives_an_unwatch() {
    let base = tempfile::tempdir().expect("a temporary store base");
    let registry = Registry::open(base.path()).expect("an empty registry");
    let placed = RootRecord {
        id: RootId::generate(SystemTime::UNIX_EPOCH),
        path: PathBuf::from("/media/disk/projects/app"),
        volume: Some(keepster_core::VolumePlace {
            identity: "uuid:1234-ABCD".to_owned(),
            within: "projects/app".to_owned(),
        }),
    };
    let unplaced = RootRecord {
        id: RootId::generate(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1)),
        path: PathBuf::from("/tmp/app"),
        volume: None,
    };
    registry.insert(&placed).expect("written");
    registry.insert(&unplaced).expect("written");

    assert_eq!(
        registry.readable().expect("read"),
        vec![placed.clone(), unplaced],
        "a volume that could not be identified is recorded as none"
    );

    registry.remove(placed.id).expect("unwatched");
    assert_eq!(registry.unwatched().expect("read"), vec![placed]);
}

#[test]
fn reading_a_registry_that_does_not_exist_creates_nothing() {
    let parent = tempfile::tempdir().expect("a temporary directory");
    let base = parent.path().join("keepster");

    let registry = Registry::at(&base);

    assert!(
        registry.held().is_err(),
        "a base that is not there cannot be listed, and saying so is the answer"
    );
    assert!(!base.exists(), "a reader leaves the disk as it found it");
}

#[test]
fn every_root_the_registry_holds_is_reported_watched_unwatched_or_unreadable() {
    let base = tempfile::tempdir().expect("a temporary store base");
    let registry = Registry::open(base.path()).expect("an empty registry");
    let watched = RootRecord {
        id: RootId::generate(SystemTime::UNIX_EPOCH),
        path: PathBuf::from("/code/watched"),
        volume: None,
    };
    let unwatched = RootRecord {
        id: RootId::generate(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1)),
        path: PathBuf::from("/code/unwatched"),
        volume: None,
    };
    let broken = RootId::generate(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2));
    registry.insert(&watched).expect("written");
    registry.insert(&unwatched).expect("written");
    registry.remove(unwatched.id).expect("unwatched");
    std::fs::create_dir_all(base.path().join(broken.to_string())).expect("a Root directory");
    std::fs::write(
        base.path().join(broken.to_string()).join("root.toml"),
        "not = [valid",
    )
    .expect("a damaged record");

    let held = Registry::at(base.path()).held().expect("listed");

    assert_eq!(
        held.iter()
            .map(|entry| (entry.id, entry.standing))
            .collect::<Vec<_>>(),
        vec![
            (watched.id, Standing::Watched),
            (unwatched.id, Standing::Unwatched),
            (broken, Standing::Watched),
        ],
        "in registration order, each with how it stands"
    );
    assert_eq!(held[0].record.as_ref().ok(), Some(&watched));
    assert_eq!(held[1].record.as_ref().ok(), Some(&unwatched));
    assert!(
        matches!(held[2].record, Err(RegistryError::Unreadable { .. })),
        "a record this build cannot read is reported, not dropped"
    );
}

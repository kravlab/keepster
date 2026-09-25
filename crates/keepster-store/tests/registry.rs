//! The Registry's one hard promise: a reader sees either the previous record or the new one.
//!
//! Asserted against real files, because the promise is made by the filesystem — a temporary file, a
//! flush, a rename — and a fake would prove nothing.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use keepster_core::RootId;
use keepster_store::{Registry, RootRecord};

#[test]
fn a_record_being_rewritten_is_never_read_half_written() {
    let base = tempfile::tempdir().expect("a temporary store base");
    let registry = Registry::open(base.path()).expect("an empty registry");

    let id = RootId::generate(SystemTime::UNIX_EPOCH);
    registry
        .insert(&RootRecord {
            id,
            path: PathBuf::from("/start"),
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

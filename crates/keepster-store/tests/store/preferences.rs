//! The UI preferences the daemon keeps (ticket 66, ticket 16): one small file beside the Roots' directories,
//! read as the defaults until something is written, and replaced atomically like a Registry record.

use keepster_store::{Preferences, PreferencesError, PreferencesStore, Registry, Theme};

/// A store base as the daemon has it: the Registry opened, so the directory exists and is private.
fn base() -> tempfile::TempDir {
    let base = tempfile::tempdir().expect("a temporary store base");
    Registry::open(base.path()).expect("the registry creates the base");
    base
}

#[test]
fn nothing_written_reads_as_the_defaults() {
    let base = base();

    let read = PreferencesStore::at(base.path())
        .read()
        .expect("no file is not an error");

    assert_eq!(
        read,
        Preferences {
            theme: Theme::System,
            syntax_line_limit: 20_000,
            wrap_lines: false,
        }
    );
    assert_eq!(read, Preferences::default());
}

#[test]
fn what_is_written_is_read_back() {
    let base = base();
    let store = PreferencesStore::at(base.path());
    let chosen = Preferences {
        theme: Theme::Dark,
        syntax_line_limit: 5_000,
        wrap_lines: true,
    };

    store.write(&chosen).expect("a write");

    assert_eq!(store.read().expect("a read"), chosen);
    assert_eq!(
        PreferencesStore::at(base.path())
            .read()
            .expect("another reader"),
        chosen,
        "the file is the store: a new reader sees what was written"
    );
}

#[test]
fn a_key_the_file_does_not_name_reads_as_its_default() {
    // A file an older build wrote, before a key existed, still reads: the missing key takes its default.
    let base = base();
    std::fs::write(base.path().join("preferences.toml"), "theme = \"light\"\n")
        .expect("a partial file");

    let read = PreferencesStore::at(base.path())
        .read()
        .expect("a partial file reads");

    assert_eq!(
        read,
        Preferences {
            theme: Theme::Light,
            ..Preferences::default()
        }
    );
}

#[test]
fn a_file_that_cannot_be_read_says_so_and_the_next_write_replaces_it() {
    let base = base();
    let file = base.path().join("preferences.toml");
    std::fs::write(&file, "theme = \"blue\"\n").expect("a file naming no theme there is");
    let store = PreferencesStore::at(base.path());

    let failure = store
        .read()
        .expect_err("an unknown theme is not guessed at");
    assert!(
        matches!(failure, PreferencesError::Unreadable { ref path, .. } if *path == file),
        "the failure names the file: {failure:?}"
    );

    store
        .write(&Preferences::default())
        .expect("a write over it");
    assert_eq!(
        store.read().expect("readable again"),
        Preferences::default()
    );
}

#[test]
fn a_write_leaves_no_temporary_file_and_no_root_behind() {
    let base = base();
    let store = PreferencesStore::at(base.path());

    store.write(&Preferences::default()).expect("a write");

    let names: Vec<_> = std::fs::read_dir(base.path())
        .expect("the base")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["preferences.toml"]);
    assert!(
        Registry::open(base.path())
            .expect("the registry")
            .list()
            .expect("a listing")
            .is_empty(),
        "the file beside the Roots' directories is not taken for a Root"
    );
}

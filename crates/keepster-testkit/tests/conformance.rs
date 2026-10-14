//! The watcher conformance report (ticket 70): what each documented event class does to a tree, which is
//! deterministic and tested here with no watcher at all, and how a report of what an operating system
//! delivered for them reads. What a platform delivers is never asserted: the report gates nothing.

use std::fs;
use std::path::Path;

use keepster_testkit::conformance::{CLASSES, Class, Delivered, Observation, render};
use keepster_testkit::host::Host;

mod common;

use common::scratch;

/// `class`, by name.
fn class(name: &str) -> &'static Class {
    CLASSES
        .iter()
        .find(|class| class.name == name)
        .unwrap_or_else(|| panic!("{name} is a class"))
}

/// A watched root and a directory beside it, on the same filesystem, after `class` was prepared and
/// acted out.
fn acted(name: &str) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let directory = scratch();
    let root = directory.path().join("root");
    let outside = directory.path().join("outside");
    fs::create_dir(&root).expect("the root");
    fs::create_dir(&outside).expect("beside it");
    let class = class(name);
    (class.prepare)(&root, &outside).expect("prepared");
    (class.act)(&root, &outside).expect("acted");
    (directory, root, outside)
}

/// The text of the file at `path`, failing the test with the path when it cannot be read.
fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_classes_are_the_ones_research_02_and_tickets_02_and_17_document() {
    let names: Vec<&str> = CLASSES.iter().map(|class| class.name).collect();

    assert_eq!(
        names,
        [
            "edit_in_place",
            "atomic_save",
            "create_file",
            "delete_file",
            "rename_within",
            "move_out",
            "new_directory_with_a_file",
            "populated_directory_moved_in",
            "write_through_a_second_hardlink",
            "permission_only",
            "directory_removed_and_created_again",
        ]
    );
    for class in CLASSES {
        assert!(!class.summary.trim().is_empty(), "{}", class.name);
        assert!(!class.source.trim().is_empty(), "{}", class.name);
    }
}

#[test]
fn an_edit_in_place_rewrites_the_file() {
    let (_directory, root, _) = acted("edit_in_place");

    assert_eq!(read(&root.join("notes.txt")), "second");
}

#[test]
fn an_atomic_save_replaces_the_file_and_leaves_no_temporary_one() {
    let (_directory, root, _) = acted("atomic_save");

    assert_eq!(read(&root.join("notes.txt")), "second");
    assert!(!root.join(".notes.txt.swp").exists());
}

#[test]
fn a_created_file_is_there() {
    let (_directory, root, _) = acted("create_file");

    assert_eq!(read(&root.join("new.txt")), "new");
}

#[test]
fn a_deleted_file_is_gone() {
    let (_directory, root, _) = acted("delete_file");

    assert!(!root.join("old.txt").exists());
}

#[test]
fn a_renamed_file_has_only_its_new_name() {
    let (_directory, root, _) = acted("rename_within");

    assert!(!root.join("before.txt").exists());
    assert_eq!(read(&root.join("after.txt")), "renamed");
}

#[test]
fn a_file_moved_out_is_beside_the_root_and_not_in_it() {
    let (_directory, root, outside) = acted("move_out");

    assert!(!root.join("leaving.txt").exists());
    assert_eq!(read(&outside.join("leaving.txt")), "leaving");
}

#[test]
fn a_new_directory_holds_its_file() {
    let (_directory, root, _) = acted("new_directory_with_a_file");

    assert_eq!(read(&root.join("src").join("new").join("lib.rs")), "// new");
}

#[test]
fn a_populated_directory_moved_in_arrives_with_its_contents() {
    let (_directory, root, outside) = acted("populated_directory_moved_in");

    assert!(!outside.join("incoming").exists());
    assert_eq!(read(&root.join("incoming").join("a.txt")), "a");
    assert_eq!(read(&root.join("incoming").join("deep").join("b.txt")), "b");
}

#[test]
fn a_write_through_the_link_outside_changes_the_file_inside() {
    let (_directory, root, outside) = acted("write_through_a_second_hardlink");

    assert_eq!(read(&root.join("linked.txt")), "second");
    assert_eq!(read(&outside.join("alias.txt")), "second");
}

#[test]
fn a_permission_only_change_leaves_the_content_alone() {
    let (_directory, root, _) = acted("permission_only");
    let path = root.join("script.sh");

    assert_eq!(read(&path), "echo");
    assert!(
        fs::metadata(&path)
            .expect("the file")
            .permissions()
            .readonly()
    );
}

#[test]
fn a_directory_removed_and_created_again_is_there_and_empty() {
    let (_directory, root, _) = acted("directory_removed_and_created_again");

    assert_eq!(
        fs::read_dir(root.join("build"))
            .expect("a directory")
            .count(),
        0
    );
}

/// A host whose every field is known, so the report's text is fixed.
fn host() -> Host {
    Host {
        os: "linux".to_owned(),
        arch: "x86_64".to_owned(),
        version: Some("6.8.0".to_owned()),
        filesystem: Some("ext4".to_owned()),
        runner: None,
    }
}

#[test]
fn a_report_names_its_machine_and_each_class_with_what_was_delivered() {
    let observations = [
        (
            class("edit_in_place"),
            Ok(Observation {
                delivered: vec![Delivered {
                    kind: "Modify(Data(Any))".to_owned(),
                    paths: vec!["notes.txt".to_owned()],
                    rescan: false,
                }],
            }),
        ),
        (
            class("populated_directory_moved_in"),
            Ok(Observation {
                delivered: Vec::new(),
            }),
        ),
    ];

    let report = render(&host(), &observations);

    assert!(
        report.contains("linux x86_64, version 6.8.0, filesystem ext4, not a hosted runner"),
        "{report}"
    );
    assert!(
        report.contains("- `Modify(Data(Any))` `notes.txt`\n"),
        "{report}"
    );
    assert!(
        report.contains("## populated_directory_moved_in\n"),
        "{report}"
    );
    assert!(report.contains("- nothing was delivered\n"), "{report}");
}

#[test]
fn a_rescan_and_a_path_outside_the_root_say_so() {
    let observations = [(
        class("move_out"),
        Ok(Observation {
            delivered: vec![Delivered {
                kind: "Other".to_owned(),
                paths: vec!["leaving.txt".to_owned(), "(outside the root)".to_owned()],
                rescan: true,
            }],
        }),
    )];

    let report = render(&host(), &observations);

    assert!(
        report.contains("- `Other` `leaving.txt`, `(outside the root)` — rescan\n"),
        "{report}"
    );
}

#[test]
fn a_class_this_machine_could_not_act_out_is_reported_and_the_rest_still_are() {
    // A filesystem without hard links, say: what the machine could not do is a finding like any other.
    let observations = [
        (
            class("write_through_a_second_hardlink"),
            Err("hard links are not supported".to_owned()),
        ),
        (
            class("create_file"),
            Ok(Observation {
                delivered: Vec::new(),
            }),
        ),
    ];

    let report = render(&host(), &observations);

    assert!(
        report.contains(
            "## write_through_a_second_hardlink\n\na file in the root changed through a hard link to it \
             outside the root. Source: research 02, F-L5.\n\n- could not be acted out: hard links are not \
             supported\n"
        ),
        "{report}"
    );
    assert!(report.contains("## create_file\n"), "{report}");
}

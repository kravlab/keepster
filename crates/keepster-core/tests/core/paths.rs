//! The one form of path parameter every endpoint takes (ticket 07).

use keepster_core::{InvalidPath, RootPath};

#[test]
fn a_relative_slash_separated_path_is_accepted_as_written() {
    let path = RootPath::parse("src/main.rs").expect("a valid path");

    assert_eq!(path.as_str(), "src/main.rs");
    assert!(!path.is_root());
}

#[test]
fn the_empty_path_is_the_root_itself() {
    assert!(RootPath::parse("").expect("the Root").is_root());
}

#[test]
fn an_absolute_path_is_refused_whatever_platform_it_came_from() {
    for text in ["/etc/passwd", "C:/Users", "c:\\Users", "\\\\server\\share"] {
        assert_eq!(RootPath::parse(text), Err(InvalidPath::Absolute), "{text}");
    }
}

#[test]
fn dot_components_are_refused_rather_than_resolved() {
    // Resolving them would give one resource two spellings, and `..` is how a read endpoint becomes a
    // read-anything endpoint.
    for text in ["./a.txt", "a/../b", "..", "a/./b", "a/.."] {
        assert_eq!(RootPath::parse(text), Err(InvalidPath::Relative), "{text}");
    }
}

#[test]
fn empty_components_are_refused() {
    for text in ["a//b", "a/", "a/b/"] {
        assert_eq!(RootPath::parse(text), Err(InvalidPath::Empty), "{text}");
    }
}

#[test]
fn a_nul_byte_is_refused() {
    assert_eq!(RootPath::parse("a\0b"), Err(InvalidPath::Nul));
}

#[test]
fn names_that_merely_start_with_dots_are_ordinary() {
    for text in [".env", "..hidden", "a/.config/x", "a:b"] {
        assert!(RootPath::parse(text).is_ok(), "{text}");
    }
}

#[test]
fn a_child_extends_the_path_by_one_component() {
    assert_eq!(RootPath::root().child("src").as_str(), "src");
    assert_eq!(
        RootPath::parse("src")
            .expect("valid")
            .child("lib.rs")
            .as_str(),
        "src/lib.rs"
    );
}

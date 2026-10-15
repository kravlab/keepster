//! The one form of path parameter every endpoint takes (ticket 07).

use keepster_core::{InvalidPath, RootPath};

#[test]
fn a_relative_slash_separated_path_is_accepted_as_written() {
    let path = RootPath::parse("src/main.rs").expect("a valid path");

    assert_eq!(path.to_str(), Some("src/main.rs"));
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
    assert_eq!(RootPath::root().child("src").to_str(), Some("src"));
    assert_eq!(
        RootPath::parse("src")
            .expect("valid")
            .child("lib.rs")
            .to_str(),
        Some("src/lib.rs")
    );
}

// Ticket 74: names are bytes. A Unix name need not be UTF-8, and the path keeps it exactly.

#[test]
fn a_name_that_is_not_utf8_is_held_as_its_exact_bytes() {
    let path = RootPath::parse_bytes(b"src/\xff.bin").expect("a valid path");

    assert_eq!(path.as_bytes(), b"src/\xff.bin");
    assert_eq!(path.to_str(), None, "it is not text");
    assert_eq!(path.to_lossy(), "src/\u{fffd}.bin");
    assert_eq!(
        path.to_string(),
        "src/\u{fffd}.bin",
        "shown as nearly as text can"
    );
    assert_eq!(path.name(), b"\xff.bin");
}

#[test]
fn a_path_given_as_bytes_is_checked_like_one_given_as_text() {
    let cases: [(&[u8], InvalidPath); 6] = [
        (b"/\xff", InvalidPath::Absolute),
        (b"C:/\xff", InvalidPath::Absolute),
        (b"\xff/../b", InvalidPath::Relative),
        (b"\xff//b", InvalidPath::Empty),
        (b"\xff/", InvalidPath::Empty),
        (b"\xff\0", InvalidPath::Nul),
    ];
    for (bytes, invalid) in cases {
        assert_eq!(RootPath::parse_bytes(bytes), Err(invalid), "{bytes:?}");
    }
    assert!(RootPath::parse_bytes(b"").expect("the Root").is_root());
}

#[test]
fn text_and_its_bytes_name_the_same_path() {
    assert_eq!(
        RootPath::parse("src/caf\u{e9}.txt"),
        RootPath::parse_bytes("src/caf\u{e9}.txt".as_bytes())
    );
}

#[test]
fn two_names_that_read_alike_as_text_stay_two_paths() {
    // Both show as U+FFFD; only their bytes tell them apart.
    let first = RootPath::parse_bytes(b"\xff").expect("valid");
    let second = RootPath::parse_bytes(b"\xfe").expect("valid");

    assert_eq!(first.to_lossy(), second.to_lossy());
    assert_ne!(first, second);
}

#[test]
fn paths_order_by_their_bytes() {
    // The order git's index keeps, so a sorted list of paths is the order a tree is built in.
    let mut paths = [
        RootPath::parse_bytes(b"\xff").expect("valid"),
        RootPath::parse("z").expect("valid"),
        RootPath::parse("a/b").expect("valid"),
        RootPath::parse("a.b").expect("valid"),
    ];
    paths.sort();

    let bytes: Vec<&[u8]> = paths.iter().map(RootPath::as_bytes).collect();
    assert_eq!(bytes, [&b"a.b"[..], b"a/b", b"z", b"\xff"]);
}

#[test]
fn a_child_parent_and_ancestors_keep_the_bytes() {
    let directory = RootPath::parse_bytes(b"\xfe").expect("valid");
    let path = directory.child(b"\xff").child("x.txt");

    assert_eq!(path.as_bytes(), b"\xfe/\xff/x.txt");
    assert_eq!(path.parent().as_bytes(), b"\xfe/\xff");
    let ancestors: Vec<Vec<u8>> = path.ancestors().map(|a| a.as_bytes().to_vec()).collect();
    assert_eq!(ancestors, [b"\xfe".to_vec(), b"\xfe/\xff".to_vec()]);
}

#[cfg(unix)]
#[test]
fn on_unix_a_path_reaches_the_file_by_its_exact_bytes() {
    use std::os::unix::ffi::OsStrExt;

    let path = RootPath::parse_bytes(b"src/\xff.bin").expect("valid");

    let native = path
        .native(std::path::Path::new("/r"))
        .expect("any name is a Unix name");
    assert_eq!(native.as_os_str().as_bytes(), b"/r/src/\xff.bin");
}

#[cfg(windows)]
#[test]
fn on_windows_a_path_reaches_the_file_with_backslashes_and_only_as_text() {
    let path = RootPath::parse("src/main.rs").expect("valid");
    assert_eq!(
        path.native(std::path::Path::new("C:\\r")),
        Some(std::path::PathBuf::from("C:\\r\\src\\main.rs"))
    );

    // Bytes that are not UTF-8 can only have come from a client: no Windows name is spelled so.
    let foreign = RootPath::parse_bytes(b"\xff").expect("valid");
    assert_eq!(foreign.native(std::path::Path::new("C:\\r")), None);
}

#[test]
fn the_root_itself_reaches_the_base() {
    assert_eq!(
        RootPath::root().native(std::path::Path::new("base")),
        Some(std::path::PathBuf::from("base"))
    );
}

#[test]
fn a_relative_platform_path_becomes_the_same_path_in_the_root() {
    let relative = std::path::Path::new("src").join("main.rs");

    assert_eq!(
        RootPath::from_native(&relative),
        Some(RootPath::parse("src/main.rs").expect("valid"))
    );
    assert_eq!(
        RootPath::from_native(std::path::Path::new("")),
        Some(RootPath::root())
    );
}

#[test]
fn a_platform_path_that_climbs_or_is_absolute_is_no_path_in_the_root() {
    assert_eq!(RootPath::from_native(std::path::Path::new("../x")), None);
    assert_eq!(
        RootPath::from_native(&std::env::temp_dir()),
        None,
        "an absolute path"
    );
}

#[cfg(unix)]
#[test]
fn on_unix_a_platform_path_keeps_its_exact_bytes() {
    use std::os::unix::ffi::OsStrExt;

    let relative = std::path::Path::new(std::ffi::OsStr::from_bytes(b"\xfe/\xff.bin"));

    assert_eq!(
        RootPath::from_native(relative),
        Some(RootPath::parse_bytes(b"\xfe/\xff.bin").expect("valid"))
    );
    assert_eq!(
        keepster_core::name_bytes(std::ffi::OsStr::from_bytes(b"\xff")),
        Some(&b"\xff"[..])
    );
}

#[cfg(windows)]
#[test]
fn on_windows_a_name_holding_an_unpaired_surrogate_has_no_bytes() {
    use std::os::windows::ffi::OsStringExt;

    let name = std::ffi::OsString::from_wide(&[0xD800, u16::from(b'x')]);

    assert_eq!(keepster_core::name_bytes(&name), None);
    assert_eq!(RootPath::from_native(std::path::Path::new(&name)), None);
}

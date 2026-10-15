//! Paths on the wire as bytes (ticket 74): a name that is not UTF-8 travels as its lossy text plus a
//! `_bytes` companion in base64url, present only where the text cannot carry it, and a request's
//! companion wins over its text.

use std::collections::BTreeMap;

use keepster_api::{
    Entry, EntryKind, PathResolution, RestoreRequest, Snapshot, encode_path_query, path_bytes,
    path_from_wire, path_text, paths_bytes, paths_from_wire, resolutions_from_wire,
    resolutions_to_wire,
};

fn entry(path: &[u8]) -> Entry {
    Entry {
        path: path_text(path),
        path_bytes: path_bytes(path),
        kind: EntryKind::File,
        size: Some(1),
        mode: None,
        permissions: None,
        binary: None,
        withheld: None,
        incomplete: None,
    }
}

#[test]
fn bytes_are_base64url_without_padding() {
    assert_eq!(path_bytes(b"\xff.txt").as_deref(), Some("_y50eHQ"));
    assert_eq!(path_bytes(b"src/\xfe").as_deref(), Some("c3JjL_4"));
    assert_eq!(
        path_from_wire("\u{fffd}.txt", Some("_y50eHQ")).as_deref(),
        Ok(&b"\xff.txt"[..])
    );
}

#[test]
fn bytes_that_are_not_base64url_are_refused() {
    assert!(
        path_from_wire("x", Some("/y50eHQ")).is_err(),
        "the standard alphabet"
    );
    assert!(path_from_wire("x", Some("_w==")).is_err(), "padding");
    assert!(path_from_wire("x", Some("!")).is_err());
}

#[test]
fn a_utf8_path_carries_no_bytes_on_the_wire() {
    let json = serde_json::to_value(entry(b"src/main.rs")).expect("serialized");

    assert_eq!(json["path"], "src/main.rs");
    assert!(json.get("path_bytes").is_none(), "{json}");
}

#[test]
fn a_path_that_is_not_utf8_carries_its_lossy_text_and_its_exact_bytes() {
    let json = serde_json::to_value(entry(b"\xff.txt")).expect("serialized");

    assert_eq!(json["path"], "\u{fffd}.txt");
    assert_eq!(json["path_bytes"], "_y50eHQ");
    let back: Entry = serde_json::from_value(json).expect("parsed");
    assert_eq!(
        path_from_wire(&back.path, back.path_bytes.as_deref()).as_deref(),
        Ok(&b"\xff.txt"[..])
    );
}

#[test]
fn a_list_of_paths_has_bytes_only_when_one_needs_them_and_then_one_per_path() {
    assert!(paths_bytes([&b"a.txt"[..], b"b.txt"]).is_empty());
    assert_eq!(
        paths_bytes([&b"a.txt"[..], b"\xff"]),
        [None, Some("_w".to_owned())]
    );

    let snapshot = Snapshot {
        id: "s".to_owned(),
        segment: "2026-10".to_owned(),
        taken_at: "2026-10-15T00:00:00.000Z".to_owned(),
        trigger: "manual".to_owned(),
        incomplete: true,
        omitted: vec!["a.txt".to_owned(), path_text(b"\xff")],
        omitted_bytes: paths_bytes([&b"a.txt"[..], b"\xff"]),
        withheld: Vec::new(),
        changed_paths: None,
        added_paths: None,
        deleted_paths: None,
        change: None,
        pinned: false,
        forgotten_paths: None,
    };
    let json = serde_json::to_value(&snapshot).expect("serialized");
    assert_eq!(json["omitted_bytes"], serde_json::json!([null, "_w"]));
}

#[test]
fn a_request_s_bytes_win_over_its_text() {
    let request: RestoreRequest = serde_json::from_value(serde_json::json!({
        "from": "s",
        "paths": ["a.txt", "\u{fffd}"],
        "paths_bytes": [null, "_w"],
    }))
    .expect("parsed");

    assert_eq!(
        paths_from_wire(&request.paths, &request.paths_bytes),
        Ok(vec![b"a.txt".to_vec(), b"\xff".to_vec()])
    );
    assert_eq!(
        paths_from_wire(&["a.txt".to_owned()], &[]),
        Ok(vec![b"a.txt".to_vec()]),
        "no bytes: the text is the path"
    );
}

#[test]
fn bytes_that_do_not_fit_the_text_are_refused() {
    assert!(paths_from_wire(&["a".to_owned(), "b".to_owned()], &[None]).is_err());
    assert!(paths_from_wire(&["a".to_owned()], &[Some("!".to_owned())]).is_err());
    assert!(path_from_wire("a", Some("/w")).is_err());
}

#[test]
fn resolutions_keyed_by_bytes_join_those_keyed_by_text() {
    let write = PathResolution {
        write: Some(true),
        ..PathResolution::default()
    };
    let keep = PathResolution {
        write: Some(false),
        ..PathResolution::default()
    };
    let text = BTreeMap::from([("a.txt".to_owned(), write.clone())]);
    let bytes = BTreeMap::from([("_w".to_owned(), keep.clone())]);

    assert_eq!(
        resolutions_from_wire(&text, &bytes),
        Ok(BTreeMap::from([
            (b"a.txt".to_vec(), write),
            (b"\xff".to_vec(), keep)
        ]))
    );
    assert!(
        resolutions_from_wire(
            &BTreeMap::new(),
            &BTreeMap::from([("!".to_owned(), PathResolution::default())])
        )
        .is_err()
    );
}

#[test]
fn resolutions_go_out_keyed_by_text_or_by_bytes_never_both() {
    let write = PathResolution {
        write: Some(true),
        ..PathResolution::default()
    };
    let paths = BTreeMap::from([
        (b"a.txt".to_vec(), write.clone()),
        (b"\xff".to_vec(), write.clone()),
    ]);

    let (text, bytes) = resolutions_to_wire(&paths);

    assert_eq!(text, BTreeMap::from([("a.txt".to_owned(), write.clone())]));
    assert_eq!(bytes, BTreeMap::from([("_w".to_owned(), write)]));
    assert_eq!(
        resolutions_from_wire(&text, &bytes),
        Ok(paths),
        "the round trip"
    );
}

#[test]
fn a_path_query_carries_every_byte() {
    assert_eq!(encode_path_query(b"src/\xff a.txt"), "src/%FF%20a.txt");
    assert_eq!(encode_path_query("src/main.rs"), "src/main.rs");
}

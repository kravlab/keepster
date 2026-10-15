//! Names as bytes at the command line (ticket 74).
//!
//! A Unix name is bytes, so a path typed here is taken as its bytes, sent with its `_bytes` companion
//! where its text cannot carry it, and printed back as the bytes it is — as `ls` and `find` print one.
//! Windows spells names in UTF-16; there a path is its text, and one no text spells (an unpaired
//! surrogate) is one the daemon Withholds and nothing here can name.

use std::ffi::OsStr;
use std::io::{self, Write};
use std::path::PathBuf;

/// The bytes a name typed here is held as: its bytes on Unix, its text elsewhere.
pub(crate) fn os_bytes(name: &OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        name.as_bytes().to_vec()
    }

    #[cfg(not(unix))]
    {
        name.to_string_lossy().into_owned().into_bytes()
    }
}

/// The Root-relative `path`, bytes, beneath `base`, spelled with this machine's separator.
pub(crate) fn native(base: PathBuf, path: &[u8]) -> PathBuf {
    if path.is_empty() {
        return base;
    }
    path.split(|byte| *byte == b'/')
        .fold(base, |joined, name| joined.join(os_name(name)))
}

/// One name's bytes as this platform spells a name: as they are on Unix, as their text elsewhere.
fn os_name(name: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;

        PathBuf::from(OsStr::from_bytes(name))
    }

    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(name).into_owned())
    }
}

/// The bytes a path on the wire names: its `_bytes` companion's, or its text's. A companion that does
/// not decode came from a daemon this build does not understand, and its text is the best name left.
pub(crate) fn exact(text: &str, bytes: Option<&str>) -> Vec<u8> {
    keepster_api::path_from_wire(text, bytes).unwrap_or_else(|_| text.as_bytes().to_vec())
}

/// A path on the wire as this platform prints a name: its exact bytes on Unix, its text elsewhere,
/// where a console takes text.
pub(crate) fn printed(text: &str, bytes: Option<&str>) -> Vec<u8> {
    if cfg!(unix) {
        exact(text, bytes)
    } else {
        text.as_bytes().to_vec()
    }
}

/// Each path of a list on the wire, beside its `_bytes` companion, as [`printed`] spells it.
pub(crate) fn printed_all(texts: &[String], bytes: &[Option<String>]) -> Vec<Vec<u8>> {
    texts
        .iter()
        .enumerate()
        .map(|(at, text)| printed(text, bytes.get(at).and_then(Option::as_deref)))
        .collect()
}

/// Printed names joined by `, `, for a line about all of them.
pub(crate) fn joined<N: std::borrow::Borrow<[u8]>>(names: &[N]) -> Vec<u8> {
    names.join(&b", "[..])
}

/// Write a path on the wire as [`printed`] spells it.
pub(crate) fn write_path(out: &mut impl Write, text: &str, bytes: Option<&str>) -> io::Result<()> {
    out.write_all(&printed(text, bytes))
}

/// `given`, typed as `PATH=VALUE`, split at its last `=`: the path as it was typed, and the value,
/// which is text — `None` without an `=`, or with a value that is not text.
pub(crate) fn split_assignment(given: &OsStr) -> Option<(PathBuf, String)> {
    let bytes = os_bytes(given);
    let at = bytes.iter().rposition(|byte| *byte == b'=')?;
    let value = std::str::from_utf8(&bytes[at + 1..]).ok()?.to_owned();

    Some((os_name(&bytes[..at]), value))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::{exact, joined, native, split_assignment, write_path};

    #[test]
    fn a_wire_path_is_its_companion_s_bytes_or_its_text() {
        assert_eq!(exact("\u{fffd}", Some("_w")), b"\xff");
        assert_eq!(exact("notes.txt", None), b"notes.txt");
        assert_eq!(exact("x", Some("!")), b"x", "an unreadable companion");
    }

    #[test]
    fn names_are_joined_by_a_comma_and_a_space() {
        assert_eq!(joined(&[b"a".to_vec(), b"\xff".to_vec()]), b"a, \xff");
        assert_eq!(joined::<Vec<u8>>(&[]), b"");
    }

    #[test]
    fn a_root_relative_path_lands_beneath_its_base() {
        assert_eq!(
            native(PathBuf::from("base"), b"a/b.txt"),
            PathBuf::from("base").join("a").join("b.txt")
        );
        assert_eq!(native(PathBuf::from("base"), b""), PathBuf::from("base"));
    }

    #[test]
    fn an_assignment_splits_at_its_last_equals_sign() {
        assert_eq!(
            split_assignment(OsStr::new("a=b.txt=snapshot")),
            Some((PathBuf::from("a=b.txt"), "snapshot".to_owned()))
        );
        assert_eq!(split_assignment(OsStr::new("snapshot")), None);
    }

    #[cfg(unix)]
    #[test]
    fn on_unix_a_name_is_taken_and_printed_as_its_bytes() {
        use std::os::unix::ffi::OsStrExt;

        assert_eq!(
            split_assignment(OsStr::from_bytes(b"\xff.sh=keep")),
            Some((
                PathBuf::from(OsStr::from_bytes(b"\xff.sh")),
                "keep".to_owned()
            ))
        );
        let mut printed = Vec::new();
        write_path(&mut printed, "\u{fffd}.txt", Some("_y50eHQ")).expect("written");
        assert_eq!(printed, b"\xff.txt");
    }
}

//! Paths as bytes on a text wire (ticket 74).
//!
//! A Root-relative path is bytes, and JSON carries text. Every field naming a path keeps its text — the
//! path itself when it is UTF-8, its lossy rendering otherwise, which is what a person reads — and gains
//! a `_bytes` companion carrying the exact bytes, in base64url without padding (RFC 4648 §5), only where
//! the text cannot carry them. The companion is additive, so `/v1` stays compatible: a client that
//! never meets such a name never sees one. In a request the companion wins over its text.
//!
//! The shapes: a scalar `X` gains `X_bytes`; a list `X` gains `X_bytes` of the same length, `null` for
//! each path that is UTF-8, and present only when one is not; a map keyed by path gains `X_bytes`,
//! keyed by the bytes.

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// Why a request's path bytes cannot be read: not base64url, or a list of them not one per path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPathBytes;

impl std::fmt::Display for InvalidPathBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the path bytes are not base64url, one for each path")
    }
}

impl std::error::Error for InvalidPathBytes {}

/// `bytes` in base64url without padding: the one spelling of a `_bytes` field.
fn encode_bytes(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The bytes a `_bytes` field spells; `None` for anything but base64url without padding.
fn decode_bytes(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

/// The text field of a path: the path itself when it is UTF-8, its lossy rendering otherwise.
pub fn path_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The `_bytes` companion of a path: present only when the path is not UTF-8.
pub fn path_bytes(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .is_err()
        .then(|| encode_bytes(bytes))
}

/// The `_bytes` companion of a list of paths: one per path, `None` for each that is UTF-8 — and empty,
/// so left off the wire, when every one is.
pub fn paths_bytes<P: AsRef<[u8]>>(paths: impl IntoIterator<Item = P>) -> Vec<Option<String>> {
    let companions: Vec<Option<String>> = paths
        .into_iter()
        .map(|path| path_bytes(path.as_ref()))
        .collect();
    if companions.iter().all(Option::is_none) {
        Vec::new()
    } else {
        companions
    }
}

/// The path a field and its companion name: the companion's bytes when there is one, the text's
/// otherwise.
pub fn path_from_wire(text: &str, bytes: Option<&str>) -> Result<Vec<u8>, InvalidPathBytes> {
    match bytes {
        Some(bytes) => decode_bytes(bytes).ok_or(InvalidPathBytes),
        None => Ok(text.as_bytes().to_vec()),
    }
}

/// The paths a list and its companion name: each companion's bytes where it has one. An empty
/// companion is none at all; any other must have one entry per path.
pub fn paths_from_wire(
    texts: &[String],
    bytes: &[Option<String>],
) -> Result<Vec<Vec<u8>>, InvalidPathBytes> {
    if bytes.is_empty() {
        return Ok(texts.iter().map(|text| text.as_bytes().to_vec()).collect());
    }
    if bytes.len() != texts.len() {
        return Err(InvalidPathBytes);
    }

    texts
        .iter()
        .zip(bytes)
        .map(|(text, bytes)| path_from_wire(text, bytes.as_deref()))
        .collect()
}

/// A map keyed by path, from its text keys and its companion keyed by base64url: one map keyed by the
/// bytes, where an entry of the companion wins over a text key naming the same bytes.
pub fn resolutions_from_wire<V: Clone>(
    text: &BTreeMap<String, V>,
    bytes: &BTreeMap<String, V>,
) -> Result<BTreeMap<Vec<u8>, V>, InvalidPathBytes> {
    let mut joined: BTreeMap<Vec<u8>, V> = text
        .iter()
        .map(|(path, value)| (path.as_bytes().to_vec(), value.clone()))
        .collect();
    for (path, value) in bytes {
        joined.insert(decode_bytes(path).ok_or(InvalidPathBytes)?, value.clone());
    }

    Ok(joined)
}

/// A map keyed by path as the wire carries it: the text keys, and the companion keyed by the bytes of
/// every key that is not UTF-8 — empty, so left off the wire, when every key is.
pub fn resolutions_to_wire<V: Clone>(
    paths: &BTreeMap<Vec<u8>, V>,
) -> (BTreeMap<String, V>, BTreeMap<String, V>) {
    let mut text = BTreeMap::new();
    let mut bytes = BTreeMap::new();
    for (path, value) in paths {
        match std::str::from_utf8(path) {
            Ok(path) => {
                text.insert(path.to_owned(), value.clone());
            }
            Err(_) => {
                bytes.insert(encode_bytes(path), value.clone());
            }
        }
    }

    (text, bytes)
}

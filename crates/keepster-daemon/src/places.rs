//! Where a path on this machine lies: inside which Watched Root, if any, compared as registering compares
//! and then by where both really are.

use std::path::{Component, Path, PathBuf};

use keepster_core::RootPath;

/// Where `path` lies inside the Root at `root`, the Root itself included — compared as registering
/// compares, and then by where both really are, so that a path spelt through a link, or not made yet
/// beneath one, still finds its Root.
pub(crate) fn inside(path: &Path, root: &Path) -> Option<RootPath> {
    relative(path, root).or_else(|| relative(&real(path)?, &real(root)?))
}

/// Where `path` really is: its nearest ancestor that exists, resolved, and the rest as written.
pub(crate) fn real(path: &Path) -> Option<PathBuf> {
    let mut missing = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            return Some(
                missing
                    .iter()
                    .rev()
                    .fold(real, |real, name| real.join(name)),
            );
        }
        missing.push(at.file_name()?);
        at = at.parent()?;
    }
}

/// `path` beneath `root`, `/`-separated, if it lies there.
fn relative(path: &Path, root: &Path) -> Option<RootPath> {
    let mut parts = Vec::new();
    for component in path.strip_prefix(root).ok()?.components() {
        match component {
            std::path::Component::Normal(name) => parts.push(name.to_str()?.to_owned()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }

    RootPath::parse(&parts.join("/")).ok()
}

/// `path` with `.` and `..` resolved by name, as the shell that typed it reads them, without touching the
/// filesystem: `..` at the top stays there.
pub(crate) fn lexical(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    resolved.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    resolved.pop();
                } else if !resolved.has_root() {
                    resolved.push(component);
                }
            }
            other => resolved.push(other),
        }
    }

    resolved
}

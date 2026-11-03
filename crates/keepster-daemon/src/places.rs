//! Where a path on this machine lies: inside which Watched Root, if any, compared as registering compares
//! and then by where both really are.

use std::path::Path;

use keepster_app::roots;
use keepster_core::RootPath;

/// Where `path` lies inside the Root at `root`, the Root itself included — compared as registering
/// compares, and then by where both really are, so that a path spelt through a link, or not made yet
/// beneath one, still finds its Root.
pub(crate) fn inside(path: &Path, root: &Path) -> Option<RootPath> {
    relative(path, root).or_else(|| {
        relative(
            &roots::real_or_nearest(path)?,
            &roots::real_or_nearest(root)?,
        )
    })
}

/// `path` beneath `root`, `/`-separated and byte for byte (ticket 74), if it lies there.
fn relative(path: &Path, root: &Path) -> Option<RootPath> {
    RootPath::from_native(path.strip_prefix(root).ok()?)
}

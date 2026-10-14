//! What the testkit's own test files share. Each test file compiles this module on its own and uses
//! only some of it, so every item allows going unused.

/// A temporary directory beside the build, where the bench puts its fixtures: never `/tmp`, which may be
/// in memory.
#[allow(dead_code)]
pub fn scratch() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("a temporary directory")
}

/// The directory holding `path`, `""` being the Root.
#[allow(dead_code)]
pub fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

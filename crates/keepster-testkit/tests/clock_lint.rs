//! ADR 0012's lint is load-bearing — a single stray clock read below the daemon would leave every
//! Retention test passing while proving nothing — so this asserts that it *fails a build*, which is
//! more than asserting that it appears in `clippy.toml`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn reading_the_clock_below_the_daemon_fails_the_build() {
    let root = repository_root();
    let fixture = root.join("fixtures/clock-violation/Cargo.toml");
    // Its own target directory, so this nested build shares no lock with the run that started it.
    let target = tempfile::tempdir().expect("a temporary target directory");

    let output = Command::new(env!("CARGO"))
        .args(["clippy", "--quiet", "--manifest-path"])
        .arg(&fixture)
        .env("CLIPPY_CONF_DIR", &root)
        .env("CARGO_TARGET_DIR", target.path())
        .output()
        .expect("`cargo clippy` must be installed: it is a contributor prerequisite");

    assert!(
        !output.status.success(),
        "clippy accepted a clock read below the daemon"
    );

    let refusal = String::from_utf8_lossy(&output.stderr);
    assert!(
        refusal.contains("disallowed"),
        "the build failed for some other reason:\n{refusal}"
    );
    assert!(
        refusal.contains("ADR 0012"),
        "the refusal must carry the reason, so whoever hits it knows what to do instead:\n{refusal}"
    );
}

/// The workspace root, two directories above this crate.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate lives two directories below the workspace root")
        .to_path_buf()
}

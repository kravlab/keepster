//! Records the build's commit hash for the version handshake.
//!
//! Emission is best-effort: a build from a source archive has no repository and must still compile. The
//! code reads the variable through `option_env!` and reports `unknown` unless it holds a full hash, so a
//! missing repository is stated rather than faked. No build timestamp is captured, which keeps the
//! binary reproducible.

use std::path::{Path, PathBuf};

fn main() {
    // The files that decide the answer must be declared as inputs, or a commit leaves a stale hash
    // embedded in the binary — and ticket 27 promises the recorded hash identifies the build. Vergen's
    // idempotent mode deliberately does not depend on the repository, which also means it does not
    // notice when the repository moves, so the watching is done here.
    watch_head();

    let Ok(git) = vergen_gitcl::GitclBuilder::default().sha(false).build() else {
        return;
    };
    let mut emitter = vergen_gitcl::Emitter::default();
    if emitter.add_instructions(&git).is_ok() {
        let _ = emitter.emit();
    }
}

/// Declare the repository files that determine the current commit.
///
/// `HEAD` changes when the branch changes; the branch's own ref file changes on every commit; and
/// `packed-refs` is where a ref lives once it has been packed, in which case no per-ref file exists.
fn watch_head() {
    let Some(git_dir) = git_dir() else {
        return;
    };

    let head = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    println!(
        "cargo:rerun-if-changed={}",
        git_dir.join("packed-refs").display()
    );

    // `HEAD` on a branch is `ref: refs/heads/<name>`; detached, it is the hash itself and needs nothing
    // further.
    if let Ok(contents) = std::fs::read_to_string(&head) {
        if let Some(reference) = contents.strip_prefix("ref: ") {
            let reference = reference.trim();
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(reference).display()
            );
        }
    }
}

/// Find the repository's git directory, walking up from this crate.
///
/// `.git` is usually a directory, but in a linked worktree or a submodule it is a file holding
/// `gitdir: <path>` — the same distinction ticket 49 has to make about capture.
fn git_dir() -> Option<PathBuf> {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR")?);

    for directory in manifest.ancestors() {
        let candidate = directory.join(".git");

        if candidate.is_dir() {
            return Some(candidate);
        }

        if candidate.is_file() {
            let contents = std::fs::read_to_string(&candidate).ok()?;
            let pointer = contents.strip_prefix("gitdir: ")?.trim();
            let pointer = Path::new(pointer);

            return Some(if pointer.is_absolute() {
                pointer.to_path_buf()
            } else {
                directory.join(pointer)
            });
        }
    }

    None
}

//! Records the build's commit hash for the version handshake.
//!
//! Emission is best-effort and idempotent: a build from a source archive has no repository, and must
//! still compile. The code reads the variable through `option_env!` and reports `unknown` when it is
//! absent, so a missing repository is stated rather than faked. No build timestamp is captured, which
//! keeps the binary reproducible.

fn main() {
    let Ok(git) = vergen_gitcl::GitclBuilder::default().sha(false).build() else {
        return;
    };
    let mut emitter = vergen_gitcl::Emitter::default();
    if emitter.idempotent().add_instructions(&git).is_ok() {
        let _ = emitter.emit();
    }
}

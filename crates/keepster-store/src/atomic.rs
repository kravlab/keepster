//! Replacing the daemon's own small files atomically: a Registry record, the UI preferences.
//!
//! These are the state git cannot reproduce, and the daemon can be killed at any point, so a reader must
//! see either the previous file or the new one, never half of one.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replace `path` with `contents`: write a temporary file beside it, flush it to disk, rename it over
/// the old one, then flush the directory.
///
/// The temporary file is the target's name with `.new` appended, so it never ends in a name a reader
/// of the directory would take for the real file.
pub(crate) fn replace_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = temporary_beside(path);
    let mut handle = File::create(&temporary)?;
    handle.write_all(contents)?;
    handle.sync_all()?;
    drop(handle);

    move_file(&temporary, path)
}

/// Move a file from one name to another in the same directory, flushing the directory afterwards.
///
/// The rename itself needs flushing, or a crash can leave the directory entry unwritten even though
/// the file's contents reached the disk. That is a POSIX notion: Windows cannot open a directory as a
/// file at all ("Access is denied"), and NTFS journals the rename itself, so there is no directory to
/// flush there.
pub(crate) fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)?;

    #[cfg(unix)]
    if let Some(parent) = to.parent() {
        File::open(parent)?.sync_all()?;
    }

    Ok(())
}

/// The name a replacement is written under before it takes `path`'s place.
fn temporary_beside(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replacement_takes_the_old_file_s_place_and_leaves_nothing_beside_it() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("state.toml");
        fs::write(&path, "old").expect("the old file");

        replace_file(&path, b"new").expect("a replacement");

        assert_eq!(fs::read_to_string(&path).expect("the file"), "new");
        let names: Vec<_> = fs::read_dir(dir.path())
            .expect("the directory")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(names, ["state.toml"]);
    }

    #[test]
    fn a_file_that_does_not_exist_yet_is_created() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("state.toml");

        replace_file(&path, b"first").expect("a first write");

        assert_eq!(fs::read_to_string(&path).expect("the file"), "first");
    }

    #[test]
    fn a_move_into_a_missing_directory_fails_and_keeps_the_source() {
        let dir = tempfile::tempdir().expect("a directory");
        let from = dir.path().join("a.toml");
        fs::write(&from, "kept").expect("the source");

        move_file(&from, &dir.path().join("missing").join("b.toml"))
            .expect_err("no such directory");

        assert_eq!(fs::read_to_string(&from).expect("the source"), "kept");
    }

    #[test]
    fn the_temporary_name_appends_to_the_whole_file_name() {
        assert_eq!(
            temporary_beside(Path::new("/base/root.toml")),
            Path::new("/base/root.toml.new")
        );
    }
}

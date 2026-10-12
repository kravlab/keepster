//! A log file rotated on size and kept to a bounded number of files (ticket 62).
//!
//! Hand-written, deliberately: no maintained crate rotates on size with a file count without bringing a
//! second level system and a second configuration surface (ticket 33). The whole requirement is here:
//! a record is never split across two files, a file grows past its size only by one record larger than
//! a file, and the total is the size times the number of files — no library caps total bytes, so neither
//! does this.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// How many rotated files are kept beside the current one: a ceiling is spread over five files.
const ROTATED: usize = 4;

/// The size of each file, and how many rotated ones are kept, for a declared `ceiling` in bytes — so
/// that the files together never hold more than it.
pub fn files_for(ceiling: u64) -> (u64, usize) {
    (ceiling / (ROTATED as u64 + 1), ROTATED)
}

/// The log: `name` in `directory`, rotated to `name.1` … `name.<keep>`, `.1` the newest.
#[derive(Debug)]
pub struct RotatingFile {
    directory: PathBuf,
    name: String,
    max_bytes: u64,
    keep: usize,
    current: Mutex<Current>,
}

#[derive(Debug)]
struct Current {
    /// The open file, or `None` while it could not be opened again after a rotation.
    file: Option<File>,
    /// What the file holds, counted rather than asked for on every record.
    size: u64,
}

impl RotatingFile {
    /// Open the log `name` in `directory`, creating both if need be, appending to what is there.
    ///
    /// `keep` is at least one: a log rotated into nothing is a log that loses a file at every rotation.
    pub fn open(directory: &Path, name: &str, max_bytes: u64, keep: usize) -> io::Result<Self> {
        if keep == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a rotated log keeps at least one rotated file",
            ));
        }
        crate::create_private_dir(directory)?;
        let path = directory.join(name);
        // A rotation interrupted between its two renames left the current file aside: it is the current
        // file still, unless a new one has been started since, in which case it is the newest rotated one.
        let aside = directory.join(format!("{name}.rotating"));
        if aside.exists() {
            let place = if path.exists() {
                directory.join(format!("{name}.1"))
            } else {
                path.clone()
            };
            if !place.exists() {
                let _ = std::fs::rename(&aside, place);
            }
        }
        let file = append(&path)?;
        let size = file.metadata()?.len();

        Ok(Self {
            directory: directory.to_path_buf(),
            name: name.to_owned(),
            max_bytes,
            keep,
            current: Mutex::new(Current {
                file: Some(file),
                size,
            }),
        })
    }

    /// Where the current file waits between the two renames of a rotation.
    fn aside(&self) -> PathBuf {
        self.directory.join(format!("{}.rotating", self.name))
    }

    /// Where the current file is.
    pub fn path(&self) -> PathBuf {
        self.directory.join(&self.name)
    }

    /// Append one record — one line — whole, rotating first if it would take the file past its size.
    ///
    /// A file is rotated only once it holds something, so a record larger than a file goes into one of
    /// its own rather than being split or dropped.
    pub fn write_record(&self, record: &[u8]) -> io::Result<()> {
        let mut current = self.current();
        let length = record.len() as u64;
        if current.file.is_none() || (current.size > 0 && current.size + length > self.max_bytes) {
            self.rotate(&mut current);
        }
        let Some(file) = current.file.as_mut() else {
            return Err(io::Error::other(format!(
                "{} could not be opened again",
                self.path().display()
            )));
        };
        file.write_all(record)?;
        current.size += length;

        Ok(())
    }

    /// Shift every file one place older and start the current one afresh.
    ///
    /// A rename can fail — on Windows whenever another process holds the file without sharing its
    /// deletion, which a reader is free to do. The log then goes on growing in the file it has and tries
    /// again at the next record: a file past its size for a while costs less than a lost line, and the
    /// bound comes back once the reader lets go.
    fn rotate(&self, current: &mut MutexGuard<'_, Current>) {
        let path = self.path();
        let numbered = |n: usize| self.directory.join(format!("{}.{n}", self.name));

        // The current file is closed before it is renamed: Windows renames no file this process holds
        // open, and a handle on a renamed file would go on writing into what is now `.1`.
        drop(current.file.take());

        // The current file is moved aside first, and the older files are shifted only once it has
        // been: a rotation that cannot rename the current file must leave every rotated file as it was,
        // since it is tried again at every record for as long as something holds the file.
        let aside = self.aside();
        let shifted = std::fs::rename(&path, &aside).and_then(|()| {
            // From the oldest: each rename replaces what is at its target, so the oldest file goes as the
            // one before it takes its place, and nothing is deleted on its own. A rotated file a reader
            // holds without sharing its deletion stops the shift where it is, and a retry at the next
            // record finds the files that already moved gone from their old places — nothing is lost
            // twice.
            let moved = (1..self.keep).rev().try_for_each(|n| {
                match std::fs::rename(numbered(n), numbered(n + 1)) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                    _ => Ok(()),
                }
            });
            // Put aside and not yet in place, the current file goes back to being the current one rather
            // than being lost, and grows on.
            moved
                .and_then(|()| std::fs::rename(&aside, numbered(1)))
                .or_else(|error| {
                    std::fs::rename(&aside, &path)?;
                    Err(error)
                })
        });

        // Open again whether or not the shift went through: a fresh file after a rotation, the same one,
        // appended to, when something held it. One that cannot be opened is tried at the next record.
        if let Ok(file) = append(&path) {
            current.size = match shifted {
                Ok(()) => 0,
                Err(_) => file
                    .metadata()
                    .map_or(current.size, |metadata| metadata.len()),
            };
            current.file = Some(file);
        }
    }

    fn current(&self) -> MutexGuard<'_, Current> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// `&RotatingFile` writes record by record, which is how a formatter hands over a line: whole, in one
/// call.
impl Write for &RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_record(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn append(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    options.open(path)
}

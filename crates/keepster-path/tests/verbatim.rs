//! Paths as this machine spells them, written the way a person types them (ticket 77).

use std::path::{Path, PathBuf};

use keepster_path::without_verbatim;

#[test]
fn a_verbatim_drive_or_share_path_is_written_the_ordinary_way() {
    // Ticket 77: Windows canonicalises to `\\?\C:\…`, which compares unequal to every path a person
    // types, so the prefix goes wherever the rest is an ordinary drive or share path.
    assert_eq!(
        without_verbatim(Path::new(r"\\?\C:\Users\dev\site")),
        PathBuf::from(r"C:\Users\dev\site")
    );
    assert_eq!(
        without_verbatim(Path::new(r"\\?\UNC\server\share\site")),
        PathBuf::from(r"\\server\share\site")
    );
}

#[test]
fn a_verbatim_path_that_is_not_a_drive_or_share_path_is_kept_as_it_is() {
    for kept in [
        r"\\?\Volume{0b8d6a3e-0000-0000-0000-100000000000}\site",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1",
        "/home/dev/site",
        r"C:\Users\dev\site",
    ] {
        assert_eq!(
            without_verbatim(Path::new(kept)),
            PathBuf::from(kept),
            "{kept}"
        );
    }
}

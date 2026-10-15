//! The symbolic name an incomplete record gives the error that kept a file out of a Snapshot (ticket 48).
//!
//! A name rather than a number, because the number means different things on different platforms and
//! the record outlives the machine that wrote it: the errno's constant in lower case on Unix, the Win32
//! error's name on Windows, and the platform-prefixed number for an error without a name here.

use std::io;

/// The symbolic name of `error`.
pub(super) fn of(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => os_error(code),
        // Not the operating system's: an error of the read itself, such as a stream cut short.
        None => snake_case(&format!("{:?}", error.kind())),
    }
}

/// `UnexpectedEof` as `unexpected_eof`.
fn snake_case(name: &str) -> String {
    let mut snake = String::with_capacity(name.len() + 4);
    for (position, character) in name.char_indices() {
        if character.is_ascii_uppercase() && position > 0 {
            snake.push('_');
        }
        snake.push(character.to_ascii_lowercase());
    }

    snake
}

#[cfg(unix)]
fn os_error(code: i32) -> String {
    let name = match code {
        libc::EACCES => "eacces",
        libc::EPERM => "eperm",
        libc::EIO => "eio",
        libc::ENOENT => "enoent",
        libc::ENOTDIR => "enotdir",
        libc::EISDIR => "eisdir",
        libc::ELOOP => "eloop",
        libc::ENXIO => "enxio",
        libc::ENODEV => "enodev",
        libc::EBUSY => "ebusy",
        libc::ETXTBSY => "etxtbsy",
        libc::EAGAIN => "eagain",
        libc::EINTR => "eintr",
        libc::EINVAL => "einval",
        libc::ENAMETOOLONG => "enametoolong",
        libc::EOVERFLOW => "eoverflow",
        libc::EFBIG => "efbig",
        libc::ESTALE => "estale",
        libc::ENOMEM => "enomem",
        libc::EMFILE => "emfile",
        libc::ENFILE => "enfile",
        libc::EDEADLK => "edeadlk",
        libc::ENOLCK => "enolck",
        libc::EILSEQ => "eilseq",
        libc::ETIMEDOUT => "etimedout",
        _ => return format!("errno_{code}"),
    };

    name.to_owned()
}

#[cfg(windows)]
fn os_error(code: i32) -> String {
    // The Win32 codes are fixed by the platform; the names are their `ERROR_` constants in lower case.
    let name = match code {
        2 => "file_not_found",
        3 => "path_not_found",
        5 => "access_denied",
        21 => "not_ready",
        23 => "crc",
        32 => "sharing_violation",
        33 => "lock_violation",
        123 => "invalid_name",
        1920 => "cant_access_file",
        _ => return format!("win32_{code}"),
    };

    name.to_owned()
}

#[cfg(not(any(unix, windows)))]
fn os_error(code: i32) -> String {
    format!("os_{code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_unix_error_is_named_by_its_errno_constant() {
        assert_eq!(of(&io::Error::from_raw_os_error(libc::EACCES)), "eacces");
        assert_eq!(of(&io::Error::from_raw_os_error(libc::EIO)), "eio");
        assert_eq!(of(&io::Error::from_raw_os_error(4095)), "errno_4095");
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_error_is_named_by_its_win32_constant() {
        assert_eq!(of(&io::Error::from_raw_os_error(32)), "sharing_violation");
        assert_eq!(of(&io::Error::from_raw_os_error(33)), "lock_violation");
        assert_eq!(of(&io::Error::from_raw_os_error(5)), "access_denied");
        assert_eq!(of(&io::Error::from_raw_os_error(9999)), "win32_9999");
    }

    #[test]
    fn an_error_that_is_not_the_systems_is_named_by_its_kind() {
        assert_eq!(
            of(&io::Error::from(io::ErrorKind::UnexpectedEof)),
            "unexpected_eof"
        );
    }
}

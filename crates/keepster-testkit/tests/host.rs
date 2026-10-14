//! The machine a report was made on (ticket 70): what its version commands print, read on every
//! platform, whichever one the tests run on.

use keepster_testkit::host::{runner, version_line, windows_version};

#[test]
fn a_version_line_is_the_text_without_its_line_break() {
    assert_eq!(version_line("14.7.1\n").as_deref(), Some("14.7.1"));
    assert_eq!(
        version_line("6.8.0-1015-azure\n").as_deref(),
        Some("6.8.0-1015-azure")
    );
}

#[test]
fn a_blank_version_line_names_no_version() {
    assert_eq!(version_line(""), None);
    assert_eq!(version_line(" \r\n"), None);
}

#[test]
fn windows_names_its_version_inside_the_brackets_ver_prints() {
    assert_eq!(
        windows_version("\r\nMicrosoft Windows [Version 10.0.20348.2700]\r\n").as_deref(),
        Some("10.0.20348.2700")
    );
}

#[test]
fn a_localised_ver_still_names_its_version() {
    // The word before the number is translated; the number is not.
    assert_eq!(
        windows_version("Microsoft Windows [Versión 10.0.19045.4529]").as_deref(),
        Some("10.0.19045.4529")
    );
}

#[test]
fn the_version_is_the_number_in_the_brackets_wherever_a_translation_puts_it() {
    assert_eq!(
        windows_version("Microsoft Windows [10.0.19045.4529 Version]").as_deref(),
        Some("10.0.19045.4529")
    );
}

#[test]
fn what_ver_prints_after_the_brackets_is_not_the_version() {
    assert_eq!(
        windows_version("Microsoft Windows [Version 10.0.20348.2700] 11\r\n").as_deref(),
        Some("10.0.20348.2700")
    );
}

#[test]
fn ver_without_a_bracketed_version_names_none() {
    assert_eq!(windows_version("Microsoft Windows"), None);
    assert_eq!(windows_version("Microsoft Windows []"), None);
    assert_eq!(windows_version("Microsoft Windows [Version"), None);
    assert_eq!(windows_version("Microsoft Windows [Version x]"), None);
}

#[test]
fn a_runner_is_named_by_its_image_and_the_image_version() {
    assert_eq!(
        runner(Some("ubuntu24"), Some("20251006.1")).as_deref(),
        Some("ubuntu24 20251006.1")
    );
    assert_eq!(runner(Some("win22"), None).as_deref(), Some("win22"));
}

#[test]
fn a_runner_name_carries_no_surrounding_whitespace() {
    assert_eq!(
        runner(Some(" ubuntu24 "), Some(" 20251006.1\n")).as_deref(),
        Some("ubuntu24 20251006.1")
    );
}

#[test]
fn a_machine_that_is_no_hosted_runner_names_none() {
    assert_eq!(runner(None, None), None);
    assert_eq!(runner(Some(""), Some(" ")), None);
    assert_eq!(runner(None, Some("20251006.1")), None);
}

//! Consent to an irreversible operation, given apart from selecting it: one mistyped flag must not
//! erase history (ticket 27).

/// The refusal an erasing meets without `--yes`. `what` says what it would have erased; the rest reads
/// the same for every command that erases (ticket 68), so a person meets one way to consent.
pub fn refusal(what: &str) -> String {
    format!("{what}; nothing was changed. Pass --yes as well to confirm")
}

#[cfg(test)]
mod tests {
    use super::refusal;

    #[test]
    fn a_refusal_says_what_was_spared_and_how_to_consent() {
        assert_eq!(
            refusal("purge erases every captured version of every Root"),
            "purge erases every captured version of every Root; nothing was changed. Pass --yes \
             as well to confirm"
        );
    }
}

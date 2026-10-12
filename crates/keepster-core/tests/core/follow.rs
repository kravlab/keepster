//! Which symlinks a capture follows, and where following stops (ticket 49): a pattern list layered like
//! the Include Rules, and stop rules that keep a followed link from leading into another Root, a History
//! Store or back into the walk itself.

use std::path::{Path, PathBuf};

use keepster_core::{
    CapturePolicy, FollowStop, Kind, Level, RootPath, Rule, RuleOrigin, RuleSet, WithheldReason,
};

fn path(text: &str) -> RootPath {
    RootPath::parse(text).expect("a valid path")
}

fn rule(text: &str, level: Level) -> Rule {
    Rule::parse(
        text,
        RootPath::root(),
        RuleOrigin {
            level,
            source: "test".to_owned(),
            line: Some(1),
        },
    )
    .expect("a valid pattern")
}

fn following(rules: &[(&str, Level)]) -> CapturePolicy {
    CapturePolicy::default().following(RuleSet::new(
        rules
            .iter()
            .map(|(text, level)| rule(text, *level))
            .collect(),
    ))
}

#[test]
fn no_link_is_followed_unless_a_pattern_opts_it_in() {
    let policy = CapturePolicy::default();

    assert!(!policy.follows(&path("docs"), Kind::Directory));
    assert!(!policy.follows(&path("notes.txt"), Kind::File));
    assert!(!policy.follows_any());
}

#[test]
fn the_follow_list_is_layered_like_the_include_rules_and_the_last_match_wins() {
    let policy = following(&[
        ("**", Level::Global),
        ("!private", Level::Global),
        ("vendor/", Level::Global),
    ]);

    assert!(policy.follows_any());
    assert!(policy.follows(&path("docs"), Kind::Directory));
    assert!(policy.follows(&path("deep/notes.txt"), Kind::File));
    assert!(!policy.follows(&path("private"), Kind::Directory));
    assert!(
        policy.follows(&path("vendor"), Kind::Directory),
        "a pattern ending in `/` names a link to a directory"
    );

    let narrower = following(&[("**", Level::Global), ("!**", Level::Root)]);
    assert!(
        !narrower.follows(&path("docs"), Kind::Directory),
        "the more specific level wins"
    );
}

fn walk(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[test]
fn following_stops_at_a_target_that_closes_a_cycle() {
    let policy = following(&[("**", Level::Global)]);
    // The Root, the targets followed so far on this branch, and the directory the link is in.
    let on_the_walk = walk(&["/home/dev/project", "/mnt/shared", "/mnt/shared/sub"]);

    for cycle in ["/home/dev/project", "/home/dev", "/mnt/shared", "/", "/mnt"] {
        assert_eq!(
            policy.stop_following(Path::new(cycle), &on_the_walk),
            Some(FollowStop::Cycle),
            "{cycle}"
        );
    }
    assert_eq!(
        policy.stop_following(Path::new("/mnt/shared/sub/deeper"), &on_the_walk),
        None,
        "a directory beneath the walk is not on it"
    );
    assert_eq!(
        policy.stop_following(Path::new("/srv/data"), &on_the_walk),
        None
    );
}

#[test]
fn following_stops_at_a_target_inside_another_root_or_a_history_store() {
    let policy = following(&[("**", Level::Global)]).with_real_fences(vec![
        (PathBuf::from("/home/dev/other"), WithheldReason::OtherRoot),
        (
            PathBuf::from("/home/dev/.local/share/keepster"),
            WithheldReason::HistoryStore,
        ),
    ]);
    let on_the_walk = walk(&["/home/dev/project"]);

    assert_eq!(
        policy.stop_following(Path::new("/home/dev/other/src"), &on_the_walk),
        Some(FollowStop::Inside(WithheldReason::OtherRoot))
    );
    assert_eq!(
        policy.stop_following(Path::new("/home/dev/.local/share/keepster"), &on_the_walk),
        Some(FollowStop::Inside(WithheldReason::HistoryStore))
    );

    // A target that *contains* one is followed, and the fence withholds that subtree once reached.
    assert_eq!(
        policy.stop_following(Path::new("/home/dev/.local"), &on_the_walk),
        None
    );
    assert_eq!(
        policy.fenced_at(Path::new("/home/dev/.local/share/keepster")),
        Some(WithheldReason::HistoryStore)
    );
    assert_eq!(policy.fenced_at(Path::new("/home/dev/.local/share")), None);
}

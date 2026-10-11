//! The trigger policy as values (ticket 43): given these events at these times, exactly these Snapshots
//! happen. No watcher, no clock and no waiting — a fold over `(time, event)` pairs.

use std::time::Duration;

use keepster_core::{Action, PolicyEvent, PolicyState, RootPath, Trigger, Triggers, decide};

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn changed(path: &str) -> PolicyEvent {
    PolicyEvent::Changed(vec![RootPath::parse(path).expect("a path")])
}

/// What a run produced: every capture with the time it was asked for, and the last timer armed.
#[derive(Debug, Default)]
struct Run {
    captures: Vec<(Duration, Trigger, Vec<String>)>,
    timer: Option<Duration>,
}

/// Feed `script` through the policy, completing each capture the moment it is asked for unless the
/// script says otherwise, and firing every armed timer that falls before the next scripted event —
/// which is what the daemon's loop does.
fn run(settings: Triggers, script: &[(u64, PolicyEvent)], until: u64) -> Run {
    run_with(settings, script, until, true)
}

fn run_with(
    settings: Triggers,
    script: &[(u64, PolicyEvent)],
    until: u64,
    complete_captures: bool,
) -> Run {
    let mut state = PolicyState::new(settings, Duration::ZERO);
    let mut run = Run::default();

    let apply = |state: PolicyState, event: PolicyEvent, now: Duration, run: &mut Run| {
        let (actions, mut state) = decide(state, event, now);
        let mut timer_set = None;
        for action in actions {
            match action {
                Action::Capture { trigger, paths } => {
                    run.captures.push((
                        now,
                        trigger,
                        paths.iter().map(|path| path.as_str().to_owned()).collect(),
                    ));
                    if complete_captures {
                        let (more, next) = decide(state, PolicyEvent::CaptureFinished, now);
                        state = next;
                        for action in more {
                            if let Action::ArmTimer { at } = action {
                                timer_set = Some(at);
                            }
                        }
                    }
                }
                Action::ArmTimer { at } => timer_set = Some(at),
            }
        }
        (state, timer_set)
    };

    // The daemon asks once at the start what is due, which is how a periodic floor arms its first timer.
    let (next, armed) = apply(state, PolicyEvent::Timer, Duration::ZERO, &mut run);
    state = next;
    let mut timer = armed;

    for (at, event) in script {
        let at = secs(*at);
        while let Some(due) = timer.filter(|due| *due <= at) {
            let (next, armed) = apply(state, PolicyEvent::Timer, due, &mut run);
            state = next;
            timer = armed;
        }
        let (next, armed) = apply(state, event.clone(), at, &mut run);
        state = next;
        if armed.is_some() {
            timer = armed;
        }
    }
    while let Some(due) = timer.filter(|due| *due <= secs(until)) {
        let (next, armed) = apply(state, PolicyEvent::Timer, due, &mut run);
        state = next;
        timer = armed;
    }

    run.timer = timer;
    run
}

fn defaults() -> Triggers {
    Triggers::default()
}

#[test]
fn the_documented_defaults_are_the_watcher_a_three_second_debounce_and_one_snapshot_a_minute() {
    let settings = defaults();

    assert!(settings.watcher);
    assert_eq!(
        settings.periodic, None,
        "the periodic floor is off by default"
    );
    assert_eq!(settings.debounce, secs(3));
    assert_eq!(settings.rate_limit, secs(60));
}

#[test]
fn an_edit_is_captured_once_the_debounce_window_has_passed() {
    let run = run(defaults(), &[(10, changed("notes.txt"))], 100);

    assert_eq!(
        run.captures,
        vec![(secs(13), Trigger::Watcher, vec!["notes.txt".to_owned()])]
    );
}

#[test]
fn edits_inside_the_window_extend_it_and_yield_one_snapshot() {
    let run = run(
        defaults(),
        &[
            (10, changed("a.txt")),
            (11, changed("b.txt")),
            (13, changed("a.txt")),
        ],
        100,
    );

    assert_eq!(
        run.captures,
        vec![(
            secs(16),
            Trigger::Watcher,
            vec!["a.txt".to_owned(), "b.txt".to_owned()]
        )]
    );
}

#[test]
fn the_rate_limit_holds_back_a_second_snapshot_and_folds_everything_into_it() {
    let run = run(
        defaults(),
        &[
            (10, changed("a.txt")),
            (20, changed("b.txt")),
            (40, changed("c.txt")),
        ],
        200,
    );

    assert_eq!(
        run.captures,
        vec![
            (secs(13), Trigger::Watcher, vec!["a.txt".to_owned()]),
            (
                secs(73),
                Trigger::Watcher,
                vec!["b.txt".to_owned(), "c.txt".to_owned()]
            ),
        ],
        "one Snapshot per minute, the second holding both later edits"
    );
}

#[test]
fn a_noisy_build_is_one_snapshot_per_rate_limit_interval_not_one_per_second() {
    let script: Vec<(u64, PolicyEvent)> = (0..300)
        .map(|second| (second, changed("target/out.o")))
        .collect();

    let run = run(
        Triggers {
            debounce: secs(1),
            ..defaults()
        },
        &script,
        400,
    );

    // A one-second window closes between every pair of writes, so only the rate limit stands between
    // the build and a Snapshot a second: one a minute, the last a minute after the one before it.
    assert_eq!(
        run.captures
            .iter()
            .map(|capture| capture.0)
            .collect::<Vec<_>>(),
        [1, 61, 121, 181, 241, 301].map(secs).to_vec()
    );
}

#[test]
fn the_rate_limit_is_configurable() {
    let run = run(
        Triggers {
            rate_limit: secs(5),
            ..defaults()
        },
        &[(10, changed("a.txt")), (14, changed("b.txt"))],
        100,
    );

    assert_eq!(
        run.captures
            .iter()
            .map(|capture| capture.0)
            .collect::<Vec<_>>(),
        vec![secs(13), secs(18)]
    );
}

#[test]
fn the_periodic_floor_captures_in_silence_when_switched_on_and_never_when_off() {
    let quiet = run(defaults(), &[], 10_000);
    assert!(quiet.captures.is_empty());
    assert_eq!(quiet.timer, None, "nothing to wait for");

    let floor = run(
        Triggers {
            periodic: Some(secs(600)),
            ..defaults()
        },
        &[],
        1_900,
    );
    assert_eq!(
        floor
            .captures
            .iter()
            .map(|capture| (capture.0, capture.1))
            .collect::<Vec<_>>(),
        vec![
            (secs(600), Trigger::Periodic),
            (secs(1200), Trigger::Periodic),
            (secs(1800), Trigger::Periodic),
        ]
    );
}

#[test]
fn changes_are_ignored_while_the_watcher_trigger_is_switched_off() {
    let run = run(
        Triggers {
            watcher: false,
            ..defaults()
        },
        &[(10, changed("a.txt"))],
        100,
    );

    assert!(run.captures.is_empty());
}

#[test]
fn a_capture_finding_the_store_locked_coalesces_into_one_snapshot_when_it_is_released() {
    let run = run_with(
        defaults(),
        &[
            (10, changed("a.txt")),
            // The daemon tried at 13 and found maintenance holding the lock.
            (13, PolicyEvent::LockBusy),
            (20, changed("b.txt")),
            (30, changed("c.txt")),
            (90, PolicyEvent::LockReleased),
            (95, PolicyEvent::CaptureFinished),
        ],
        200,
        false,
    );

    assert_eq!(
        run.captures,
        vec![
            (secs(13), Trigger::Watcher, vec!["a.txt".to_owned()]),
            (
                secs(90),
                Trigger::Watcher,
                vec!["a.txt".to_owned(), "b.txt".to_owned(), "c.txt".to_owned()]
            ),
        ],
        "the deferred capture is retried once, covering everything since"
    );
}

#[test]
fn edits_during_a_capture_are_kept_for_the_next_one() {
    let run = run_with(
        Triggers {
            rate_limit: secs(1),
            ..defaults()
        },
        &[
            (10, changed("a.txt")),
            (14, changed("b.txt")),
            (15, PolicyEvent::CaptureFinished),
            (30, PolicyEvent::CaptureFinished),
        ],
        100,
        false,
    );

    assert_eq!(
        run.captures,
        vec![
            (secs(13), Trigger::Watcher, vec!["a.txt".to_owned()]),
            (secs(17), Trigger::Watcher, vec!["b.txt".to_owned()]),
        ]
    );
}

#[test]
fn a_failed_capture_keeps_its_paths_for_the_next_attempt() {
    let run = run_with(
        defaults(),
        &[
            (10, changed("a.txt")),
            (14, PolicyEvent::CaptureFailed),
            (80, PolicyEvent::CaptureFinished),
        ],
        200,
        false,
    );

    assert_eq!(
        run.captures,
        vec![
            (secs(13), Trigger::Watcher, vec!["a.txt".to_owned()]),
            (secs(73), Trigger::Watcher, vec!["a.txt".to_owned()]),
        ],
        "retried once the rate limit allows"
    );
}

#[test]
fn shutdown_flushes_a_pending_window_at_once_bypassing_the_rate_limit() {
    let run = run(
        defaults(),
        &[
            (10, changed("a.txt")),
            (20, changed("b.txt")),
            (21, PolicyEvent::Shutdown),
            (22, changed("c.txt")),
        ],
        1_000,
    );

    assert_eq!(
        run.captures,
        vec![
            (secs(13), Trigger::Watcher, vec!["a.txt".to_owned()]),
            (secs(21), Trigger::Watcher, vec!["b.txt".to_owned()]),
        ],
        "the flush ignores the minute since the last Snapshot, and nothing is accepted after it"
    );
    assert_eq!(run.timer, None);
}

#[test]
fn shutdown_waits_for_the_capture_in_flight_then_flushes() {
    let mut state = PolicyState::new(defaults(), Duration::ZERO);
    let (actions, next) = decide(state, changed("a.txt"), secs(0));
    state = next;
    assert!(matches!(actions.as_slice(), [Action::ArmTimer { .. }]));
    let (actions, next) = decide(state, PolicyEvent::Timer, secs(3));
    state = next;
    assert!(matches!(actions.as_slice(), [Action::Capture { .. }]));
    let (_, next) = decide(state, changed("b.txt"), secs(4));
    state = next;

    let (actions, next) = decide(state, PolicyEvent::Shutdown, secs(5));
    state = next;
    assert!(actions.is_empty(), "the capture in flight finishes first");
    assert!(!state.stopped());

    let (actions, next) = decide(state, PolicyEvent::CaptureFinished, secs(6));
    state = next;
    assert!(matches!(actions.as_slice(), [Action::Capture { .. }]));
    assert!(!state.stopped());

    let (actions, state) = decide(state, PolicyEvent::CaptureFinished, secs(7));
    assert!(actions.is_empty());
    assert!(state.stopped(), "nothing is left to flush");
}

#[test]
fn a_shutdown_with_nothing_pending_stops_at_once() {
    let state = PolicyState::new(defaults(), Duration::ZERO);

    let (actions, state) = decide(state, PolicyEvent::Shutdown, secs(5));

    assert!(actions.is_empty());
    assert!(state.stopped());
}

#[test]
fn new_settings_apply_to_the_window_already_counting_down() {
    let run = run(
        defaults(),
        &[
            (10, changed("a.txt")),
            (
                11,
                PolicyEvent::Settings(Triggers {
                    debounce: secs(30),
                    ..defaults()
                }),
            ),
        ],
        100,
    );

    assert_eq!(run.captures[0].0, secs(40));
}

#[test]
fn a_duration_is_written_with_its_unit_and_shown_the_way_it_would_be_written() {
    use keepster_core::{format_duration, parse_duration};

    assert_eq!(parse_duration("3s"), Ok(secs(3)));
    assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
    assert_eq!(parse_duration("10m"), Ok(secs(600)));
    assert_eq!(parse_duration(" 1h "), Ok(secs(3600)));
    assert_eq!(parse_duration("2d"), Ok(secs(2 * 86_400)));
    for refused in ["3", "s", "1.5s", "3 sec", "-1s", ""] {
        assert!(parse_duration(refused).is_err(), "{refused:?} is refused");
    }

    assert_eq!(format_duration(secs(3)), "3s");
    assert_eq!(format_duration(secs(60)), "1m");
    assert_eq!(format_duration(secs(5400)), "90m");
    assert_eq!(format_duration(Duration::from_millis(1500)), "1500ms");
    assert_eq!(format_duration(Duration::ZERO), "0ms");
}

#[test]
fn a_file_written_more_often_than_the_debounce_is_still_captured_once_per_rate_limit() {
    // A log or a watch-mode build never goes quiet for three seconds; waiting for it to would never
    // capture it at all. The rate limit is what caps how often, so it also bounds how long a change
    // waits.
    let script: Vec<(u64, PolicyEvent)> = (0..200)
        .map(|second| (second, changed("build.log")))
        .collect();

    let run = run(defaults(), &script, 300);

    assert_eq!(
        run.captures
            .iter()
            .map(|capture| capture.0)
            .collect::<Vec<_>>(),
        [60, 120, 180, 240].map(secs).to_vec()
    );
}

#[test]
fn a_capture_that_wrote_nothing_does_not_hold_back_the_next_one() {
    let run = run_with(
        defaults(),
        &[
            (10, changed("touched.txt")),
            // The daemon found nothing to write: the file was only touched.
            (14, PolicyEvent::CaptureUnchanged),
            (20, changed("edited.txt")),
            (24, PolicyEvent::CaptureFinished),
        ],
        200,
        false,
    );

    assert_eq!(
        run.captures
            .iter()
            .map(|capture| capture.0)
            .collect::<Vec<_>>(),
        vec![secs(13), secs(23)],
        "no Snapshot was taken at 13, so the rate limit does not count from it"
    );
}

#[test]
fn a_periodic_capture_that_could_not_happen_is_retried_as_periodic() {
    let run = run_with(
        Triggers {
            watcher: false,
            periodic: Some(secs(600)),
            ..defaults()
        },
        &[
            (600, PolicyEvent::LockBusy),
            (700, PolicyEvent::LockReleased),
            (705, PolicyEvent::CaptureFinished),
        ],
        1_000,
        false,
    );

    assert_eq!(
        run.captures
            .iter()
            .map(|capture| (capture.0, capture.1))
            .collect::<Vec<_>>(),
        vec![
            (secs(600), Trigger::Periodic),
            (secs(700), Trigger::Periodic)
        ]
    );
}

#[test]
fn a_flush_that_fails_while_stopping_is_not_retried() {
    let state = PolicyState::new(defaults(), Duration::ZERO);
    let (_, state) = decide(state, changed("a.txt"), secs(0));
    let (actions, state) = decide(state, PolicyEvent::Shutdown, secs(1));
    assert!(matches!(actions.as_slice(), [Action::Capture { .. }]));

    let (actions, state) = decide(state, PolicyEvent::CaptureFailed, secs(2));

    assert!(
        actions.is_empty(),
        "a stopping daemon tries once, then stops"
    );
    assert!(state.stopped());
}

#[test]
fn a_reconciliation_is_captured_as_one_and_takes_any_pending_change_with_it() {
    let run = run(
        defaults(),
        &[(10, changed("a.txt")), (11, PolicyEvent::Reconcile)],
        100,
    );

    assert_eq!(
        run.captures,
        vec![(secs(11), Trigger::Reconcile, vec!["a.txt".to_owned()])],
        "a pass walks everything, so the change waiting for its window rides along"
    );
}

#[test]
fn a_reconciliation_waits_for_the_rate_limit_like_any_other_snapshot() {
    let run = run(
        defaults(),
        &[(10, changed("a.txt")), (20, PolicyEvent::Reconcile)],
        200,
    );

    assert_eq!(
        run.captures
            .iter()
            .map(|capture| (capture.0, capture.1))
            .collect::<Vec<_>>(),
        vec![(secs(13), Trigger::Watcher), (secs(73), Trigger::Reconcile)]
    );
}

#[test]
fn a_reconciliation_is_owed_even_with_every_trigger_switched_off() {
    let run = run(
        Triggers {
            watcher: false,
            ..defaults()
        },
        &[(5, PolicyEvent::Reconcile)],
        100,
    );

    assert_eq!(run.captures.len(), 1);
    assert_eq!(run.captures[0].1, Trigger::Reconcile);
}

#[test]
fn a_reconciliation_that_could_not_happen_is_retried_as_one() {
    let run = run_with(
        defaults(),
        &[
            (5, PolicyEvent::Reconcile),
            (6, PolicyEvent::LockBusy),
            (50, PolicyEvent::LockReleased),
            (51, PolicyEvent::CaptureFinished),
        ],
        200,
        false,
    );

    assert_eq!(
        run.captures
            .iter()
            .map(|capture| (capture.0, capture.1))
            .collect::<Vec<_>>(),
        vec![
            (secs(5), Trigger::Reconcile),
            (secs(50), Trigger::Reconcile)
        ]
    );
}

/// Feed one event, returning what it asked for.
fn step(state: PolicyState, event: PolicyEvent, at: u64) -> (Vec<Action>, PolicyState) {
    decide(state, event, secs(at))
}

#[test]
fn an_incomplete_pass_is_retried_with_a_doubling_backoff_until_one_is_complete() {
    // ADR 0004: a carried-forward entry looks unchanged and no second event is coming, so the Root is
    // passed over again — whatever Triggers are active, as Reconciliation is.
    let quiet = Triggers {
        watcher: false,
        ..defaults()
    };
    let state = PolicyState::new(quiet, Duration::ZERO);

    let (actions, state) = step(state, PolicyEvent::PassOutcome { incomplete: true }, 0);
    assert_eq!(actions, [Action::ArmTimer { at: secs(30) }]);

    let (actions, state) = step(state, PolicyEvent::Timer, 30);
    assert!(
        matches!(
            &actions[..],
            [Action::Capture {
                trigger: Trigger::Retry,
                ..
            }]
        ),
        "{actions:?}"
    );

    let (actions, state) = step(state, PolicyEvent::PassOutcome { incomplete: true }, 31);
    assert!(actions.is_empty(), "the capture is still being reported");
    let (actions, state) = step(state, PolicyEvent::CaptureUnchanged, 31);
    assert_eq!(
        actions,
        [Action::ArmTimer { at: secs(91) }],
        "still incomplete: twice as long"
    );

    let (_, state) = step(state, PolicyEvent::Timer, 91);
    let (_, state) = step(state, PolicyEvent::PassOutcome { incomplete: false }, 92);
    let (actions, _) = step(state, PolicyEvent::CaptureUnchanged, 92);
    assert!(
        actions.is_empty(),
        "a complete pass owes nothing: {actions:?}"
    );
}

#[test]
fn the_retry_backoff_stops_doubling_at_an_hour_and_starts_again_after_a_complete_pass() {
    let quiet = Triggers {
        watcher: false,
        ..defaults()
    };
    let mut state = PolicyState::new(quiet, Duration::ZERO);
    let mut now = 0;
    let mut waits = Vec::new();
    for _ in 0..10 {
        let (_, next) = step(state, PolicyEvent::PassOutcome { incomplete: true }, now);
        let (actions, next) = step(next, PolicyEvent::CaptureUnchanged, now);
        let [Action::ArmTimer { at }] = actions[..] else {
            panic!("a retry is armed: {actions:?}");
        };
        waits.push(at.as_secs() - now);
        now = at.as_secs();
        let (_, next) = step(next, PolicyEvent::Timer, now);
        state = next;
    }
    assert_eq!(waits, [30, 60, 120, 240, 480, 960, 1920, 3600, 3600, 3600]);

    let (_, state) = step(state, PolicyEvent::PassOutcome { incomplete: false }, now);
    let (_, state) = step(state, PolicyEvent::CaptureUnchanged, now);
    let (actions, _) = step(
        state,
        PolicyEvent::PassOutcome { incomplete: true },
        now + 1,
    );
    assert_eq!(actions, [Action::ArmTimer { at: secs(now + 31) }]);
}

#[test]
fn the_daemon_can_shorten_the_first_retry() {
    let state = PolicyState::new(defaults(), Duration::ZERO).retrying_after(secs(1));

    let (actions, _) = step(state, PolicyEvent::PassOutcome { incomplete: true }, 0);

    assert_eq!(actions, [Action::ArmTimer { at: secs(1) }]);
}

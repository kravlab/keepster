//! How the configuration levels merge into one Root's effective configuration, and where each value
//! came from (ticket 42).
//!
//! The merge seam: the files are given as text, so each test states a configuration and reads back the
//! effective value and its provenance without a daemon in between. What reaches the API from here is
//! covered by the daemon's own tests.

use std::path::Path;

use keepster_app::config::{
    Effective, Entry, GlobalConfig, RootConfig, Setting, Value, effective, global_effective,
};
use keepster_core::{DEFAULT_MAX_SIZE, Level, RootId};

const GLOBAL: &str = "/home/someone/.config/keepster/config.toml";
const HOME: &str = "/home/someone";
const ROOT_PATH: &str = "/home/someone/code/app";

fn id() -> RootId {
    "01BX5ZZKBKACTAV9WEVGEMMVRZ".parse().expect("an identity")
}

fn global(text: &str) -> GlobalConfig {
    GlobalConfig::parse(Path::new(GLOBAL), text, Some(Path::new(HOME)))
        .unwrap_or_else(|error| panic!("the global configuration parses: {error}"))
}

fn own(text: &str) -> RootConfig {
    RootConfig::parse(&Path::new(ROOT_PATH).join(".keepster.toml"), text)
        .unwrap_or_else(|error| panic!("the Root's file parses: {error}"))
}

fn merged(global: &GlobalConfig, own: Option<&RootConfig>) -> Effective {
    effective(global, id(), Path::new(ROOT_PATH), own)
}

fn setting<'a>(effective: &'a Effective, key: &str) -> &'a Setting {
    &effective
        .keys
        .iter()
        .find(|found| found.spec.name == key)
        .unwrap_or_else(|| panic!("`{key}` is in the effective configuration"))
        .setting
}

fn scalar<'a>(effective: &'a Effective, key: &str) -> &'a Entry {
    match setting(effective, key) {
        Setting::Scalar(entry) => entry,
        Setting::List(_) => panic!("`{key}` is a scalar"),
    }
}

fn list<'a>(effective: &'a Effective, key: &str) -> &'a [Entry] {
    match setting(effective, key) {
        Setting::List(entries) => entries,
        Setting::Scalar(_) => panic!("`{key}` is a list"),
    }
}

fn patterns(entries: &[Entry]) -> Vec<(&str, Level)> {
    entries
        .iter()
        .map(|entry| match &entry.value {
            Value::Pattern(pattern) => (pattern.as_str(), entry.origin.level),
            other => panic!("{other:?} is not a pattern"),
        })
        .collect()
}

#[test]
fn a_user_who_configures_nothing_gets_the_documented_defaults() {
    let effective = merged(&global(""), None);

    let max_size = scalar(&effective, "files.max_size");
    assert_eq!(max_size.value, Value::Bytes(DEFAULT_MAX_SIZE));
    assert_eq!(max_size.origin.level, Level::Default);

    let respect = scalar(&effective, "files.respect_gitignore");
    assert_eq!(respect.value, Value::Bool(false));
    assert_eq!(respect.origin.level, Level::Default);

    assert!(list(&effective, "files.exclude").is_empty());
    assert!(list(&effective, "files.secret_opt_in").is_empty());
    assert!(effective.warnings.is_empty());
}

#[test]
fn each_level_replaces_a_scalar_set_by_a_less_specific_one() {
    let only_global = global("[files]\nmax_size = \"1 MiB\"\n");
    let max_size = scalar(&merged(&only_global, None), "files.max_size").clone();
    assert_eq!(max_size.value, Value::Bytes(1024 * 1024));
    assert_eq!(max_size.origin.level, Level::Global);
    assert_eq!(max_size.origin.source, GLOBAL);
    assert_eq!(max_size.origin.line, Some(2));

    let with_pattern = global(
        "[files]\nmax_size = \"1 MiB\"\n\n[[roots]]\npath = \"~/code/*\"\n[roots.files]\nmax_size = \"2 MiB\"\n",
    );
    let max_size = scalar(&merged(&with_pattern, None), "files.max_size").clone();
    assert_eq!(max_size.value, Value::Bytes(2 * 1024 * 1024));
    assert_eq!(max_size.origin.level, Level::GlobalPattern);
    assert_eq!(max_size.origin.line, Some(7));

    let with_id = global(
        "[root.01BX5ZZKBKACTAV9WEVGEMMVRZ.files]\nmax_size = \"3 MiB\"\n\n[[roots]]\npath = \"~/code/*\"\n[roots.files]\nmax_size = \"2 MiB\"\n",
    );
    let max_size = scalar(&merged(&with_id, None), "files.max_size").clone();
    assert_eq!(
        max_size.value,
        Value::Bytes(3 * 1024 * 1024),
        "an id block outranks a pattern block, wherever it is written"
    );
    assert_eq!(max_size.origin.level, Level::GlobalRoot);

    let file = own("[files]\nmax_size = \"4 MiB\"\n");
    let max_size = scalar(&merged(&with_id, Some(&file)), "files.max_size").clone();
    assert_eq!(max_size.value, Value::Bytes(4 * 1024 * 1024));
    assert_eq!(max_size.origin.level, Level::Root);
    assert!(max_size.origin.source.ends_with(".keepster.toml"));
}

#[test]
fn a_root_matching_several_pattern_blocks_takes_the_last_one_alone() {
    let config = global(concat!(
        "[[roots]]\npath = \"~/code/**\"\n[roots.files]\nmax_size = \"1 MiB\"\nexclude = [\"*.log\"]\n\n",
        "[[roots]]\npath = \"~/code/*\"\n[roots.files]\nmax_size = \"2 MiB\"\n\n",
        "[[roots]]\npath = \"/elsewhere/*\"\n[roots.files]\nmax_size = \"9 MiB\"\n",
    ));

    let effective = merged(&config, None);

    let max_size = scalar(&effective, "files.max_size");
    assert_eq!(max_size.value, Value::Bytes(2 * 1024 * 1024));
    assert_eq!(max_size.origin.line, Some(10));
    assert!(
        list(&effective, "files.exclude").is_empty(),
        "the broader block written first contributes nothing, its lists included"
    );
}

#[test]
fn pattern_lists_layer_so_a_more_specific_level_can_negate_a_rule() {
    let config = global(concat!(
        "[files]\nexclude = [\"*.log\", \"build/\"]\n\n",
        "[[roots]]\npath = \"/home/someone/code/app\"\n[roots.files]\nexclude = [\"*.tmp\"]\n",
    ));
    let file = own("[files]\nexclude = [\"!keep.log\"]\n");

    let effective = merged(&config, Some(&file));

    assert_eq!(
        patterns(list(&effective, "files.exclude")),
        vec![
            ("*.log", Level::Global),
            ("build/", Level::Global),
            ("*.tmp", Level::GlobalPattern),
            ("!keep.log", Level::Root),
        ]
    );
    assert_eq!(
        setting(&effective, "files.exclude").origin().level,
        Level::Root
    );

    let keep = keepster_core::RootPath::parse("keep.log").expect("a path");
    let other = keepster_core::RootPath::parse("other.log").expect("a path");
    assert!(
        effective
            .policy
            .rules()
            .classify(&keep, keepster_core::Kind::File)
            .captured()
    );
    assert!(
        !effective
            .policy
            .rules()
            .classify(&other, keepster_core::Kind::File)
            .captured()
    );
}

#[test]
fn a_root_file_setting_a_global_only_key_is_ignored_with_a_warning() {
    let file = own("[files]\nsecret_opt_in = [\".env\"]\nexclude = [\"*.log\"]\n");

    let effective = merged(&global(""), Some(&file));

    assert!(list(&effective, "files.secret_opt_in").is_empty());
    assert_eq!(
        list(&effective, "files.exclude").len(),
        1,
        "the rest of the file applies"
    );
    assert_eq!(effective.warnings.len(), 1);
    assert_eq!(effective.warnings[0].code.as_str(), "key_ignored");
    assert!(
        effective.warnings[0]
            .message
            .contains("files.secret_opt_in")
    );
}

#[test]
fn the_global_file_may_opt_one_root_in_past_the_denylist() {
    let config = global("[root.01BX5ZZKBKACTAV9WEVGEMMVRZ.files]\nsecret_opt_in = [\".env\"]\n");

    let effective = merged(&config, None);

    assert_eq!(
        patterns(list(&effective, "files.secret_opt_in")),
        vec![(".env", Level::GlobalRoot)]
    );
    let other = effective_for_other_root(&config);
    assert!(list(&other, "files.secret_opt_in").is_empty());
}

fn effective_for_other_root(config: &GlobalConfig) -> Effective {
    let other: RootId = "01BX5ZZKBKACTAV9WEVGEMMVS0".parse().expect("an identity");
    effective(config, other, Path::new(ROOT_PATH), None)
}

#[test]
fn a_typo_or_a_bad_block_makes_the_whole_file_invalid() {
    for (text, expected) in [
        ("[files]\nexlude = [\"*.log\"]\n", "exlude"),
        ("[fils]\nexclude = [\"*.log\"]\n", "fils"),
        ("[root.not-an-id.files]\nexclude = []\n", "not-an-id"),
        ("[[roots]]\npath = \"code/*\"\n", "absolute"),
        (
            "[[roots]]\npath = \"~/code/*\"\n[roots.files]\nmax_size = \"lots\"\n",
            "max_size",
        ),
    ] {
        let error =
            GlobalConfig::parse(Path::new(GLOBAL), text, Some(Path::new(HOME))).expect_err(text);
        assert!(
            error.message.contains(expected),
            "{text:?} is refused naming {expected}: {}",
            error.message
        );
        assert_eq!(error.path, Path::new(GLOBAL));
    }

    let error = RootConfig::parse(Path::new("/r/.keepster.toml"), "[root.x.files]\n")
        .expect_err("a Root's own file holds no per-Root blocks");
    assert!(error.message.contains("root"), "{}", error.message);
}

#[test]
fn the_triggers_default_to_the_watcher_alone_and_a_set_is_replaced_wholesale() {
    let defaults = merged(&global(""), None);
    assert_eq!(defaults.triggers, keepster_core::Triggers::default());
    assert_eq!(
        scalar(&defaults, "triggers.active").value,
        Value::Words(vec!["watcher".to_owned()])
    );
    assert_eq!(
        scalar(&defaults, "triggers.debounce").value,
        Value::Duration(std::time::Duration::from_secs(3))
    );

    let config = global(
        "[triggers]\nactive = [\"watcher\", \"periodic\"]\nperiodic = \"30m\"\nrate_limit = \"5m\"\n",
    );
    let file = own("[triggers]\nactive = [\"periodic\"]\ndebounce = \"10s\"\n");
    let effective = merged(&config, Some(&file));

    assert!(
        !effective.triggers.watcher,
        "the Root's set replaces the global one, it does not add to it"
    );
    assert_eq!(
        effective.triggers.periodic,
        Some(std::time::Duration::from_secs(1800))
    );
    assert_eq!(
        effective.triggers.debounce,
        std::time::Duration::from_secs(10)
    );
    assert_eq!(
        effective.triggers.rate_limit,
        std::time::Duration::from_secs(300)
    );
    assert_eq!(
        scalar(&effective, "triggers.active").origin.level,
        Level::Root
    );
    assert_eq!(
        scalar(&effective, "triggers.rate_limit").origin.level,
        Level::Global
    );
}

#[test]
fn a_trigger_setting_that_cannot_be_honoured_is_refused() {
    for (text, expected) in [
        ("[triggers]\nactive = [\"inotify\"]\n", "inotify"),
        ("[triggers]\ndebounce = \"3\"\n", "debounce"),
        ("[triggers]\nrate_limit = \"0s\"\n", "rate_limit"),
        ("[triggers]\nperiodic = \"0s\"\n", "periodic"),
        ("[triggers]\nwatcher = true\n", "watcher"),
    ] {
        let error = GlobalConfig::parse(Path::new(GLOBAL), text, None).expect_err(text);
        assert!(
            error.message.contains(expected),
            "{text:?} is refused naming {expected}: {}",
            error.message
        );
    }
}

mod held {
    //! The configuration the daemon holds across edits: reload, the last known-good, and the queue of
    //! Roots whose classification changed.

    use std::fs;
    use std::path::{Path, PathBuf};

    use keepster_app::config::{Configuration, ROOT_CONFIG, Setting, Value};
    use keepster_core::RootId;
    use tempfile::TempDir;

    struct Fixture {
        _directory: TempDir,
        global: PathBuf,
        state: PathBuf,
        root: PathBuf,
    }

    fn fixture() -> Fixture {
        let directory = TempDir::new().expect("a temporary directory");
        let base = directory.path().to_path_buf();
        let state = base.join("store");
        let root = base.join("project");
        fs::create_dir_all(state.join(id().to_string())).expect("a Root's store directory");
        fs::create_dir_all(&root).expect("a Root");

        Fixture {
            global: base.join("config.toml"),
            state,
            root,
            _directory: directory,
        }
    }

    fn id() -> RootId {
        "01BX5ZZKBKACTAV9WEVGEMMVRZ".parse().expect("an identity")
    }

    fn configuration(fixture: &Fixture) -> Configuration {
        Configuration::new(Some(fixture.global.clone()), None, fixture.state.clone())
    }

    fn max_size(configuration: &mut Configuration, root: &Path) -> Option<u64> {
        let resolved = configuration.resolve(id(), root);
        resolved.effective.map(|effective| {
            match &effective
                .keys
                .iter()
                .find(|key| key.spec.name == "files.max_size")
                .expect("max_size")
                .setting
            {
                Setting::Scalar(entry) => match entry.value {
                    Value::Bytes(bytes) => bytes,
                    ref other => panic!("{other:?}"),
                },
                Setting::List(_) => panic!("a scalar"),
            }
        })
    }

    #[test]
    fn an_edit_to_either_file_is_seen_at_the_next_resolution() {
        let fixture = fixture();
        let mut configuration = configuration(&fixture);
        assert_eq!(
            max_size(&mut configuration, &fixture.root),
            Some(keepster_core::DEFAULT_MAX_SIZE)
        );

        fs::write(&fixture.global, "[files]\nmax_size = \"1 KiB\"\n").expect("the global file");
        assert_eq!(max_size(&mut configuration, &fixture.root), Some(1024));

        fs::write(
            fixture.root.join(ROOT_CONFIG),
            "[files]\nmax_size = \"2 KiB\"\n",
        )
        .expect("the Root's file");
        assert_eq!(max_size(&mut configuration, &fixture.root), Some(2048));
    }

    #[test]
    fn an_invalid_edit_keeps_the_last_known_good_in_force_and_says_why() {
        let fixture = fixture();
        let mut configuration = configuration(&fixture);
        fs::write(&fixture.global, "[files]\nmax_size = \"1 KiB\"\n").expect("the global file");
        assert_eq!(max_size(&mut configuration, &fixture.root), Some(1024));

        fs::write(&fixture.global, "[files]\nmax_size = \"lots\"\n").expect("a typo");
        let resolved = configuration.resolve(id(), &fixture.root);

        assert_eq!(resolved.errors.len(), 1);
        assert_eq!(resolved.errors[0].path, fixture.global);
        assert_eq!(max_size(&mut configuration, &fixture.root), Some(1024));

        fs::write(&fixture.global, "[files]\nmax_size = \"3 KiB\"\n").expect("the fix");
        let resolved = configuration.resolve(id(), &fixture.root);
        assert!(resolved.errors.is_empty());
        assert_eq!(max_size(&mut configuration, &fixture.root), Some(3072));
    }

    #[test]
    fn the_last_known_good_survives_a_restart() {
        let fixture = fixture();
        {
            let mut configuration = configuration(&fixture);
            fs::write(
                fixture.root.join(ROOT_CONFIG),
                "[files]\nmax_size = \"1 KiB\"\n",
            )
            .expect("the Root's file");
            assert_eq!(max_size(&mut configuration, &fixture.root), Some(1024));
        }
        fs::write(fixture.root.join(ROOT_CONFIG), "[files\n").expect("a broken file");

        let mut restarted = configuration(&fixture);
        let resolved = restarted.resolve(id(), &fixture.root);

        assert_eq!(resolved.errors.len(), 1);
        assert_eq!(max_size(&mut restarted, &fixture.root), Some(1024));
    }

    #[test]
    fn with_no_known_good_version_there_is_no_effective_configuration() {
        let fixture = fixture();
        fs::write(&fixture.global, "[files]\nexclude = 3\n").expect("a broken file");

        let resolved = configuration(&fixture).resolve(id(), &fixture.root);

        assert!(resolved.effective.is_none());
        assert_eq!(resolved.errors.len(), 1);
    }

    #[test]
    fn a_change_to_what_classifies_paths_queues_the_root_for_a_full_comparison() {
        let fixture = fixture();
        let mut configuration = configuration(&fixture);
        configuration.resolve(id(), &fixture.root);
        assert!(configuration.take_reconcile_queue().is_empty());

        // The same rules on other lines classify nothing differently.
        fs::write(&fixture.global, "\n\n[files]\n").expect("the global file");
        configuration.resolve(id(), &fixture.root);
        assert!(configuration.take_reconcile_queue().is_empty());

        fs::write(&fixture.global, "[files]\nexclude = [\"*.log\"]\n").expect("a new rule");
        configuration.resolve(id(), &fixture.root);
        assert_eq!(
            configuration
                .take_reconcile_queue()
                .into_iter()
                .collect::<Vec<_>>(),
            vec![id()]
        );
        assert!(
            configuration.take_reconcile_queue().is_empty(),
            "taking the queue empties it"
        );
    }
    #[test]
    fn a_directory_that_is_no_root_is_configured_as_registering_it_would_and_names_what_its_file_sets()
     {
        let fixture = fixture();
        let candidate = fixture.root.parent().expect("a parent").join("candidate");
        fs::create_dir_all(&candidate).expect("a directory");
        fs::write(
            candidate.join(ROOT_CONFIG),
            "[files]\nexclude = [\"*.tmp\"]\nmax_size = \"1 MiB\"\n",
        )
        .expect("its own file");
        let mut configuration = configuration(&fixture);

        let effective = configuration
            .candidate(&candidate, None)
            .expect("a configuration");

        assert!(
            effective
                .policy
                .rules()
                .rules()
                .any(|rule| rule.text == "*.tmp"),
            "its own file is honoured"
        );
        assert_eq!(
            keepster_app::config::set_by_root_file(&effective)
                .into_iter()
                .map(|(key, values)| (key, values.into_iter().cloned().collect::<Vec<_>>()))
                .collect::<Vec<_>>(),
            vec![
                ("files.exclude", vec![Value::Pattern("*.tmp".to_owned())]),
                ("files.max_size", vec![Value::Bytes(1024 * 1024)]),
            ],
            "and what it sets is named"
        );
    }

    #[test]
    fn a_directory_once_watched_is_configured_under_the_identity_registering_it_again_takes_back() {
        let fixture = fixture();
        fs::write(
            &fixture.global,
            format!("[root.{}.files]\nmax_size = \"3 MiB\"\n", id()),
        )
        .expect("the global file");
        let mut configuration = configuration(&fixture);

        let fresh = configuration
            .candidate(&fixture.root, None)
            .expect("a configuration");
        let again = configuration
            .candidate(&fixture.root, Some(id()))
            .expect("a configuration");

        let max_size = |effective: &keepster_app::config::Effective| {
            effective
                .keys
                .iter()
                .find(|key| key.spec.name == "files.max_size")
                .map(|key| {
                    key.setting
                        .values()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                })
        };
        let block = Some(vec![Value::Bytes(3 * 1024 * 1024)]);
        assert_ne!(max_size(&fresh), block, "no block names a new identity");
        assert_eq!(
            max_size(&again),
            block,
            "the block for the identity it takes back applies"
        );
    }

    #[test]
    fn a_directory_once_watched_keeps_out_what_forget_excluded_from_it() {
        let fixture = fixture();
        fs::write(
            fixture.state.join(id().to_string()).join("forgotten.toml"),
            "patterns = [\".env.local\"]\n",
        )
        .expect("what Forget excluded");
        let mut configuration = configuration(&fixture);

        let fresh = configuration
            .candidate(&fixture.root, None)
            .expect("a configuration");
        let again = configuration
            .candidate(&fixture.root, Some(id()))
            .expect("a configuration");

        let forgets = |effective: &keepster_app::config::Effective| {
            effective
                .policy
                .rules()
                .rules()
                .any(|rule| rule.text == ".env.local")
        };
        assert!(!forgets(&fresh), "nothing was forgotten from a new Root");
        assert!(
            forgets(&again),
            "registering it again takes back what Forget excluded"
        );
    }
}

#[test]
fn reconciliation_runs_at_start_and_trusts_the_stat_filter_unless_told_otherwise() {
    let defaults = merged(&global(""), None);
    assert!(defaults.reconcile.at_start);
    assert_eq!(defaults.reconcile.verify, keepster_core::Verify::Stat);
    assert_eq!(
        scalar(&defaults, "reconcile.verify").value,
        Value::Name("stat".to_owned())
    );

    let file = own("[reconcile]\nat_start = false\nverify = \"hash\"\n");
    let effective = merged(&global(""), Some(&file));
    assert!(!effective.reconcile.at_start);
    assert_eq!(effective.reconcile.verify, keepster_core::Verify::Hash);
    assert_eq!(
        scalar(&effective, "reconcile.at_start").origin.level,
        Level::Root
    );

    let error = GlobalConfig::parse(Path::new(GLOBAL), "[reconcile]\nverify = \"mtime\"\n", None)
        .expect_err("an unknown mode");
    assert!(
        error.message.contains("reconcile.verify"),
        "{}",
        error.message
    );
}

#[test]
fn a_restore_asks_about_modes_unless_the_configuration_answers() {
    let defaults = merged(&global(""), None);
    assert_eq!(defaults.restore_modes, None, "asking is the default");
    assert_eq!(
        scalar(&defaults, "restore.modes").value,
        Value::Name("ask".to_owned())
    );

    let file = own("[restore]\nmodes = \"keep\"\n");
    let effective = merged(&global("[restore]\nmodes = \"snapshot\"\n"), Some(&file));
    assert_eq!(
        effective.restore_modes,
        Some(keepster_core::ModeChoice::Keep)
    );
    assert_eq!(
        scalar(&effective, "restore.modes").origin.level,
        Level::Root,
        "a Root's own file may answer it"
    );

    let error = GlobalConfig::parse(Path::new(GLOBAL), "[restore]\nmodes = \"umask\"\n", None)
        .expect_err("an unknown answer");
    assert!(error.message.contains("restore.modes"), "{}", error.message);
}

#[test]
fn the_retention_ladder_is_the_users_to_set_and_must_be_one() {
    use keepster_core::{Keep, Ladder, Tier};
    use std::time::Duration;

    let hour = Duration::from_secs(3600);
    let defaults = merged(&global(""), None);
    assert_eq!(defaults.retention, Ladder::default());

    let ladder = "[[retention.tier]]\nage = \"2d\"\nkeep = \"all\"\n\n\
                  [[retention.tier]]\nage = \"14d\"\nkeep = \"6h\"\n\n\
                  [[retention.tier]]\nage = \"90d\"\nkeep = \"weekly\"\n";
    let effective = merged(&global(ladder), None);
    assert_eq!(
        effective.retention.tiers(),
        [
            Tier {
                age: 48 * hour,
                keep: Keep::All
            },
            Tier {
                age: 14 * 24 * hour,
                keep: Keep::Every(6 * hour)
            },
            Tier {
                age: 90 * 24 * hour,
                keep: Keep::Every(7 * 24 * hour)
            },
        ]
    );
    assert_eq!(
        scalar(&effective, "retention.tier").origin.level,
        Level::Global
    );

    let file = own("[[retention.tier]]\nage = \"1h\"\nkeep = \"all\"\n");
    let shortened = merged(&global(""), Some(&file));
    assert_eq!(
        shortened.retention,
        Ladder::default(),
        "a Root's own file cannot shorten the history"
    );
    assert!(
        shortened
            .warnings
            .iter()
            .any(|warning| warning.message.contains("retention.tier")),
        "{:?}",
        shortened.warnings
    );

    for (written, said) in [
        (
            "[[retention.tier]]\nage = \"7d\"\nkeep = \"all\"\n\n[[retention.tier]]\nage = \"1d\"\nkeep = \"daily\"\n",
            "older",
        ),
        (
            "[[retention.tier]]\nage = \"7d\"\nkeep = \"daily\"\n",
            "first Tier must keep `all`",
        ),
        (
            "[[retention.tier]]\nage = \"7d\"\nkeep = \"fortnightly\"\n",
            "not what a Tier keeps",
        ),
        (
            "[[retention.tier]]\nage = \"1d\"\nkeep = \"all\"\n\n[[retention.tier]]\nage = \"7d\"\nkeep = \"2h\"\n\n[[retention.tier]]\nage = \"9d\"\nkeep = \"3h\"\n",
            "whole periods",
        ),
    ] {
        let error = GlobalConfig::parse(Path::new(GLOBAL), written, None).expect_err(said);
        assert!(
            error.message.contains("retention.tier") && error.message.contains(said),
            "{}",
            error.message
        );
    }
}

#[test]
fn the_git_maintenance_runs_with_is_named_at_the_top_of_the_global_file_alone() {
    // Absolute on the platform the test runs on: `/opt/...` is not, on Windows.
    let absolute = if cfg!(windows) {
        r"C:\Program Files\Git\cmd\git.exe"
    } else {
        "/opt/git/bin/git"
    };
    let named = GlobalConfig::parse(
        Path::new(GLOBAL),
        &format!("[git]\npath = '{absolute}'\n"),
        None,
    );
    assert!(named.is_ok(), "{named:?}");

    let relative = GlobalConfig::parse(Path::new(GLOBAL), "[git]\npath = \"git\"\n", None)
        .expect_err("a relative path would be looked up");
    assert!(
        relative.message.contains("absolute"),
        "{}",
        relative.message
    );

    let in_a_block = GlobalConfig::parse(
        Path::new(GLOBAL),
        "[root.01K6DX3QR7M2V8C9WZ4PN5YQ1A.git]\npath = \"/usr/bin/git\"\n",
        None,
    );
    assert!(in_a_block.is_err(), "one daemon runs one git");
    assert!(
        RootConfig::parse(Path::new(".keepster.toml"), "[git]\npath = \"/tmp/evil\"\n").is_err(),
        "a Root's own file never chooses what the daemon runs"
    );
}

#[test]
fn the_global_level_alone_is_the_defaults_under_the_global_file_s_own_table() {
    let global = global(
        "[log]\nformat = \"json\"\n\n[[roots]]\npath = \"~/code/*\"\n[roots.files]\nmax_size = \"2 MiB\"\n\n[root.01BX5ZZKBKACTAV9WEVGEMMVRZ.files]\nmax_size = \"3 MiB\"\n",
    );

    let level = global_effective(&global);

    let format = scalar(&level, "log.format");
    assert_eq!(format.value, Value::Name("json".to_owned()));
    assert_eq!(format.origin.level, Level::Global, "set by the global file");
    let size = scalar(&level, "files.max_size");
    assert_eq!(
        (size.value.clone(), size.origin.level),
        (Value::Bytes(DEFAULT_MAX_SIZE), Level::Default),
        "a pattern block and an id block each describe some Roots, never the global level"
    );
}

mod as_read {
    //! The configuration as its files read now, for a diagnostics bundle collected without the daemon
    //! (ticket 63): the same merge, and nothing written — no known-good copy kept or fallen back on.

    use std::fs;

    use keepster_app::config::{
        ROOT_CONFIG, Setting, Value, global_effective, read_global, read_root, starting_settings,
    };
    use keepster_core::Level;
    use tempfile::TempDir;

    fn id() -> keepster_core::RootId {
        "01BX5ZZKBKACTAV9WEVGEMMVRZ".parse().expect("an identity")
    }

    #[test]
    fn a_daemon_starting_now_would_take_its_settings_from_the_global_file_s_own_table() {
        let directory = TempDir::new().expect("a temporary directory");
        let global_path = directory.path().join("config.toml");
        fs::write(&global_path, "[log]\nformat = \"json\"\n").expect("the global file");
        let global = read_global(Some(&global_path), None).expect("the global file reads");

        let settings = starting_settings(Some(&global));

        assert_eq!(settings.log_format, "json");
        assert_eq!(settings.log_level, "info", "an absent key at its default");
    }

    #[test]
    fn with_no_global_file_to_read_a_daemon_starting_now_would_take_the_defaults() {
        let settings = starting_settings(None);

        assert_eq!(settings.log_format, "text");
        assert!(settings.listener);
    }

    #[test]
    fn reading_the_files_for_a_report_merges_them_and_writes_nothing() {
        let directory = TempDir::new().expect("a temporary directory");
        let global_path = directory.path().join("config.toml");
        let state = directory.path().join("store");
        let root = directory.path().join("project");
        fs::create_dir_all(state.join(id().to_string())).expect("a Root's store directory");
        fs::create_dir_all(&root).expect("a Root");
        fs::write(&global_path, "[log]\nformat = \"json\"\n").expect("the global file");
        fs::write(root.join(ROOT_CONFIG), "[files]\nmax_size = \"3 MiB\"\n").expect("its file");
        fs::write(
            state.join(id().to_string()).join("forgotten.toml"),
            "patterns = [\".env\"]\n",
        )
        .expect("what Forget excluded");

        let global = read_global(Some(&global_path), None).expect("the global file reads");
        let resolved = read_root(&global, id(), &root, &state);

        let level = global_effective(&global);
        let format = level
            .keys
            .iter()
            .find(|key| key.spec.name == "log.format")
            .expect("log.format");
        assert!(
            matches!(&format.setting, Setting::Scalar(entry) if entry.value == Value::Name("json".to_owned())),
            "{format:?}"
        );
        let effective = resolved.effective.expect("in force");
        assert!(resolved.errors.is_empty(), "{:?}", resolved.errors);
        let setting = |name: &str| {
            &effective
                .keys
                .iter()
                .find(|key| key.spec.name == name)
                .expect("the key")
                .setting
        };
        assert!(
            matches!(setting("files.max_size"), Setting::Scalar(entry) if entry.origin.level == Level::Root),
            "the Root's own file is read"
        );
        assert!(
            matches!(setting("files.exclude"), Setting::List(entries) if entries.iter().any(|entry| entry.origin.level == Level::Forget)),
            "and what Forget excluded from now on"
        );
        let written: Vec<_> = fs::read_dir(state.join(id().to_string()))
            .expect("listed")
            .map(|entry| entry.expect("an entry").file_name())
            .collect();
        assert_eq!(written, ["forgotten.toml"], "no known-good copy is kept");
        assert!(!state.join("config.known-good.toml").exists());
    }

    #[test]
    fn a_file_that_does_not_parse_is_the_answer_not_a_fallback() {
        let directory = TempDir::new().expect("a temporary directory");
        let global_path = directory.path().join("config.toml");
        fs::write(&global_path, "[files\n").expect("a broken global file");

        let error = read_global(Some(&global_path), None).expect_err("refused");

        assert_eq!(error.path, global_path);
    }

    #[test]
    fn no_global_file_is_the_defaults() {
        let global = read_global(None, None).expect("the defaults");

        assert!(
            global_effective(&global)
                .keys
                .iter()
                .all(|key| key.setting.origin().level == Level::Default)
        );
    }
}

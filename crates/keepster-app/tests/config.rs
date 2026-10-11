//! How the configuration levels merge into one Root's effective configuration, and where each value
//! came from (ticket 42).
//!
//! The merge seam: the files are given as text, so each test states a configuration and reads back the
//! effective value and its provenance without a daemon in between. What reaches the API from here is
//! covered by the daemon's own tests.

use std::path::Path;

use keepster_app::config::{Effective, Entry, GlobalConfig, RootConfig, Setting, Value, effective};
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

//! A diagnostics bundle's files and inventory (ticket 63, ADR 0013): fixed content, every file listed
//! with where it came from and what it discloses, and a bundle collected without the daemon naming every
//! section it lacks rather than looking whole.

use std::time::{Duration, SystemTime};

use keepster_api::{
    DiagnosticConfiguration, Diagnostics, GlobalConfiguration, LogFileTail, LogTail,
};
use keepster_daemon::bundle::{Bundle, DiskEnvironment, DiskWatches, OnDisk};
use keepster_testkit::Harness;

use crate::jobs::captured;

/// Every file a bundle the daemon collected holds besides the inventory, in the order it holds them.
const FROM_THE_DAEMON: [&str; 7] = [
    "versions.json",
    "status.json",
    "environment.json",
    "configuration.json",
    "registry.json",
    "topology.json",
    "watermarks.json",
];

async fn from_the_daemon() -> Diagnostics {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    captured(&daemon).await;
    let answer = daemon.get("/v1/diagnostics").await;
    serde_json::from_slice(&answer.body).expect("the diagnostics")
}

fn on_disk() -> OnDisk {
    OnDisk {
        environment: DiskEnvironment {
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            filesystems: Vec::new(),
            watches: DiskWatches {
                ceiling: Some(4096),
            },
        },
        configuration: DiagnosticConfiguration {
            global: GlobalConfiguration {
                file: None,
                keys: Vec::new(),
                errors: Vec::new(),
            },
            roots: Vec::new(),
        },
        unread_roots: None,
        registry: Ok(Vec::new()),
        log: LogTail {
            format: "json".to_owned(),
            files: vec![LogFileTail {
                name: "daemon.log".to_owned(),
                size: 12,
                text: "{\"a\":\"b\"}\n".to_owned(),
                truncated: true,
            }],
            error: None,
        },
    }
}

fn missing(bundle: &Bundle) -> Vec<(&str, &str)> {
    bundle
        .missing()
        .iter()
        .map(|missing| (missing.name.as_str(), missing.reason.as_str()))
        .collect()
}

fn names(bundle: &Bundle) -> Vec<&str> {
    bundle
        .files()
        .iter()
        .map(|file| file.name.as_str())
        .collect()
}

#[tokio::test]
async fn a_bundle_the_daemon_collected_holds_every_section_and_says_what_each_discloses() {
    let bundle = Bundle::from_daemon(keepster_daemon::version_info(), from_the_daemon().await);

    assert_eq!(names(&bundle), FROM_THE_DAEMON);
    assert!(
        bundle.files().iter().all(|file| !file.source.is_empty()
            && !file.discloses.is_empty()
            && !file.content.is_empty()),
        "every file says where it came from and what it reveals"
    );
    let inventory = bundle.inventory();
    for name in FROM_THE_DAEMON.iter().chain(&["inventory.txt"]) {
        assert!(inventory.contains(name), "`{name}` is listed:\n{inventory}");
    }
    for file in bundle.files() {
        assert!(
            inventory.contains(&file.discloses),
            "what `{}` discloses is printed:\n{inventory}",
            file.name
        );
    }
    assert!(
        inventory.contains("Log format: text"),
        "which format the log is in:\n{inventory}"
    );
    assert_eq!(
        bundle
            .missing()
            .iter()
            .map(|missing| missing.name.as_str())
            .collect::<Vec<_>>(),
        ["logs/"],
        "a daemon in a test process writes no log, and the bundle says so"
    );
}

#[tokio::test]
async fn versions_name_the_cli_and_the_daemon_apart() {
    let bundle = Bundle::from_daemon(keepster_daemon::version_info(), from_the_daemon().await);

    let versions: serde_json::Value =
        serde_json::from_slice(&bundle.files()[0].content).expect("JSON");
    assert_eq!(versions["cli"]["api_version"], "v1", "{versions}");
    assert_eq!(versions["daemon"]["api_version"], "v1", "{versions}");
}

#[test]
fn a_bundle_collected_without_the_daemon_names_every_section_it_lacks() {
    let bundle = Bundle::from_disk(
        keepster_daemon::version_info(),
        on_disk(),
        "no daemon is listening",
    );

    assert_eq!(
        names(&bundle),
        [
            "versions.json",
            "environment.json",
            "configuration.json",
            "registry.json",
            "logs/daemon.log",
        ]
    );
    let missing: Vec<&str> = bundle
        .missing()
        .iter()
        .map(|missing| missing.name.as_str())
        .collect();
    for lacking in ["status.json", "topology.json", "watermarks.json"] {
        assert!(missing.contains(&lacking), "`{lacking}`: {missing:?}");
    }
    assert!(
        missing
            .iter()
            .any(|name| name.starts_with("environment.json")),
        "the parts of the environment only the daemon knows: {missing:?}"
    );
    assert!(
        bundle
            .missing()
            .iter()
            .all(|missing| missing.reason.contains("no daemon is listening")),
        "each says why"
    );
    let inventory = bundle.inventory();
    assert!(inventory.contains("status.json"), "{inventory}");
    assert!(inventory.contains("no daemon is listening"), "{inventory}");
    assert!(inventory.contains("Log format: json"), "{inventory}");

    let environment: serde_json::Value =
        serde_json::from_slice(&bundle.files()[1].content).expect("JSON");
    assert!(
        environment.get("git").is_none() && environment.get("maintenance").is_none(),
        "what was not collected is absent, not empty: {environment}"
    );
    assert!(
        environment["watches"].get("in_use").is_none(),
        "{environment}"
    );
}

#[test]
fn the_archive_holds_the_inventory_and_every_file_as_listed() {
    let bundle = Bundle::from_disk(
        keepster_daemon::version_info(),
        on_disk(),
        "no daemon is listening",
    );
    let mut archive = Vec::new();

    bundle
        .write_zip(
            &mut archive,
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_776_800),
        )
        .expect("written");

    // Every entry is stored, not compressed, so each file's bytes appear in the archive as they are.
    let contains = |needle: &[u8]| archive.windows(needle.len()).any(|window| window == needle);
    assert!(contains(bundle.inventory().as_bytes()), "the inventory");
    for file in bundle.files() {
        assert!(contains(file.name.as_bytes()), "`{}` is named", file.name);
        assert!(contains(&file.content), "`{}` is held whole", file.name);
    }
    assert_eq!(&archive[..4], b"PK\x03\x04", "a zip archive");
}

#[test]
fn a_bundle_collected_without_the_daemon_names_the_root_configurations_it_could_not_read() {
    let found = OnDisk {
        unread_roots: Some("the global file says `[log` and ends".to_owned()),
        ..on_disk()
    };

    let bundle = Bundle::from_disk(
        keepster_daemon::version_info(),
        found,
        "no daemon is listening",
    );

    assert!(
        missing(&bundle).contains(&(
            "configuration.json (each Root's configuration)",
            "the global file says `[log` and ends"
        )),
        "{:?}",
        missing(&bundle)
    );
    assert!(
        bundle
            .inventory()
            .contains("configuration.json (each Root's configuration): the global file says"),
        "{}",
        bundle.inventory()
    );
}

#[test]
fn without_the_daemon_the_log_format_is_read_from_the_log_s_own_lines() {
    let mut found = on_disk();
    // The configuration file says one thing; the log was written in another.
    found.log.format = "text".to_owned();

    let bundle = Bundle::from_disk(
        keepster_daemon::version_info(),
        found,
        "no daemon is listening",
    );

    let inventory = bundle.inventory();
    assert!(
        inventory.contains("Log format: json, read from the log's own lines"),
        "{inventory}"
    );
    assert!(
        bundle.files()[4].discloses.contains("in the json format"),
        "{}",
        bundle.files()[4].discloses
    );
}

#[test]
fn with_no_log_line_to_read_it_from_the_format_is_the_configuration_s_and_says_so() {
    let mut found = on_disk();
    found.log.files.clear();

    let bundle = Bundle::from_disk(
        keepster_daemon::version_info(),
        found,
        "no daemon is listening",
    );

    assert!(
        bundle
            .inventory()
            .contains("Log format: json, as the configuration file reads now"),
        "{}",
        bundle.inventory()
    );
}

#[tokio::test]
async fn a_log_that_could_not_be_read_is_named_missing_with_why() {
    let mut found = from_the_daemon().await;
    found.log.error = Some("daemon.log: permission denied".to_owned());

    let bundle = Bundle::from_daemon(keepster_daemon::version_info(), found);

    assert_eq!(
        missing(&bundle),
        [(
            "logs/",
            "the log could not be read: daemon.log: permission denied"
        )],
        "never \"no log file\" when there is one"
    );
}

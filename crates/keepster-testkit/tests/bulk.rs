//! The bulk generator of ticket 30: a seeded tree whose shape is declared, not incidentally plausible.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use keepster_core::{RootPath, Verdict};
use keepster_testkit::bulk::{
    CEILING, EXCLUSION, Excluded, FLAT, PlannedFile, Profile, SMALL, SizeBucket, TreePlan, content,
    materialize, plan,
};

mod common;

use common::parent_of;

#[test]
fn the_small_profile_plans_exactly_its_declared_counts() {
    let tree = plan(&SMALL);

    assert_eq!(tree.files.len(), 5_000);
    assert_eq!(tree.directories.len(), 500);
    assert_eq!(total(&tree), 50 * 1024 * 1024);
    // The excluded directory itself, and the 2,000 beneath it.
    assert_eq!(tree.excluded_directories.len(), 2_001);
    assert_eq!(tree.excluded_files.len(), 20_000);
}

#[test]
fn the_ceiling_profile_plans_exactly_v1s_per_root_claim() {
    let tree = plan(&CEILING);

    assert_eq!(tree.files.len(), 100_000);
    assert_eq!(tree.directories.len(), 10_000);
    assert_eq!(total(&tree), 10 * 1024 * 1024 * 1024);
    assert_eq!(tree.excluded_directories.len(), 10_001);
    assert_eq!(tree.excluded_files.len(), 100_000);
}

#[test]
fn the_flat_profile_plans_every_included_file_empty_in_the_root() {
    let tree = plan(&FLAT);

    assert_eq!(tree.files.len(), 20_000);
    assert!(tree.directories.is_empty());
    assert!(tree.files.iter().all(|file| !file.path.contains('/')));
    // Empty, so every file is one blob and a first capture costs the walk and the index, not the writes:
    // the cost a broken insertion order multiplies.
    assert!(tree.files.iter().all(|file| file.size == 0));
    // Every fixture has its excluded directory; the flat one's holds a single file, so it is not empty.
    assert_eq!(tree.excluded_directories, ["node_modules"]);
    assert_eq!(tree.excluded_files.len(), 1);
}

#[test]
fn the_exclusion_profile_plans_a_small_included_tree_beside_a_large_excluded_one() {
    let tree = plan(&EXCLUSION);

    assert_eq!(tree.files.len(), 100);
    assert_eq!(tree.directories.len(), 10);
    assert_eq!(total(&tree), 100 * 1024);
    assert_eq!(tree.excluded_directories.len(), 2_001);
    assert_eq!(tree.excluded_files.len(), 20_000);
}

#[test]
fn one_seed_plans_one_tree() {
    assert_eq!(plan(&SMALL), plan(&SMALL));

    let reseeded = Profile {
        seed: SMALL.seed + 1,
        ..SMALL
    };
    assert_ne!(plan(&SMALL).files, plan(&reseeded).files);
}

#[test]
fn every_bucket_holds_its_share_of_the_files_and_no_size_leaves_its_bucket() {
    let declared = [
        (SMALL, vec![2_500, 1_750, 650, 100]),
        (CEILING, vec![45_000, 30_000, 20_000, 4_500, 500]),
    ];
    for (profile, counts) in declared {
        assert_eq!(
            held_per_bucket(&profile, &plan(&profile)),
            counts,
            "{}",
            profile.name
        );
    }
}

#[test]
fn sizes_drawn_short_of_the_total_grow_to_it_within_their_bucket() {
    // A hundred draws from 0..=100 average about 5,000 bytes, well short of 9,000.
    let profile = Profile {
        files: 100,
        bytes: 9_000,
        sizes: &[SizeBucket {
            permille: 1000,
            smallest: 0,
            largest: 100,
        }],
        ..tiny()
    };
    let tree = plan(&profile);

    assert_eq!(total(&tree), 9_000);
    assert!(tree.files.iter().all(|file| file.size <= 100));
}

#[test]
fn the_files_rounding_leaves_over_go_to_the_first_bucket() {
    // Half of seven is three and a half: three each, and the seventh to the first bucket.
    let profile = Profile {
        files: 7,
        bytes: 60,
        sizes: &[
            SizeBucket {
                permille: 500,
                smallest: 0,
                largest: 10,
            },
            SizeBucket {
                permille: 500,
                smallest: 11,
                largest: 20,
            },
        ],
        ..tiny()
    };
    let tree = plan(&profile);

    assert_eq!(held_per_bucket(&profile, &tree), vec![4, 3]);
    assert_eq!(total(&tree), 60);
}

#[test]
fn no_directory_fans_out_beyond_the_profile_and_each_follows_its_parent() {
    for profile in [SMALL, CEILING] {
        let tree = plan(&profile);
        let mut included: HashMap<&str, usize> = HashMap::new();
        let mut listed: HashSet<&str> = HashSet::new();
        for directory in &tree.directories {
            let parent = parent_of(directory);
            assert!(
                parent.is_empty() || listed.contains(parent),
                "{}: {directory} is listed before its parent",
                profile.name
            );
            listed.insert(directory);
            *included.entry(parent).or_default() += 1;
        }
        // The excluded subtree is never watched, so it spends nothing of the fan-out; it still lists
        // every directory after its parent.
        for directory in tree.excluded_directories.iter().skip(1) {
            assert!(
                listed.contains(parent_of(directory))
                    || parent_of(directory) == profile.excluded.name,
                "{}: {directory} is listed before its parent",
                profile.name
            );
            listed.insert(directory);
        }

        assert_eq!(
            included.values().max(),
            Some(&profile.fan_out),
            "{}",
            profile.name
        );
    }
}

#[test]
fn no_planned_directory_is_empty() {
    // An empty directory is a degenerate case of its own (ticket 25): it belongs in the catalogue, not
    // inside the bulk fixture, where it would be measured without being declared.
    for profile in [SMALL, CEILING] {
        let tree = plan(&profile);
        let mut occupied: HashSet<&str> = HashSet::new();
        for path in tree
            .files
            .iter()
            .chain(&tree.excluded_files)
            .map(|file| file.path.as_str())
            .chain(tree.directories.iter().map(String::as_str))
            .chain(tree.excluded_directories.iter().map(String::as_str))
        {
            occupied.insert(parent_of(path));
        }

        let empty: Vec<&String> = tree
            .directories
            .iter()
            .chain(&tree.excluded_directories)
            .filter(|directory| !occupied.contains(directory.as_str()))
            .collect();
        assert!(empty.is_empty(), "{}: empty {empty:?}", profile.name);
    }
}

#[test]
fn the_excluded_subtree_holds_exactly_its_declared_paths_and_nothing_included_is_inside_it() {
    for profile in [SMALL, CEILING] {
        let tree = plan(&profile);
        let inside = |path: &str| path.starts_with("node_modules/") || path == "node_modules";

        assert!(tree.excluded_directories.iter().all(|path| inside(path)));
        assert!(
            tree.excluded_files
                .iter()
                .all(|file| inside(&file.path) && file.size == 1024)
        );
        assert!(!tree.directories.iter().any(|path| inside(path)));
        assert!(!tree.files.iter().any(|file| inside(&file.path)));
    }
}

#[test]
fn no_path_is_planned_twice() {
    for profile in [SMALL, CEILING] {
        let tree = plan(&profile);
        let mut paths: HashSet<&str> = HashSet::new();
        for path in tree
            .files
            .iter()
            .chain(&tree.excluded_files)
            .map(|file| file.path.as_str())
            .chain(tree.directories.iter().map(String::as_str))
            .chain(tree.excluded_directories.iter().map(String::as_str))
        {
            assert!(paths.insert(path), "{}: {path} twice", profile.name);
        }
    }
}

#[test]
#[should_panic(expected = "must sum to 1000‰")]
fn shares_that_do_not_sum_to_the_whole_are_refused() {
    plan(&Profile {
        sizes: &[SizeBucket {
            permille: 999,
            smallest: 0,
            largest: 100,
        }],
        ..tiny()
    });
}

#[test]
#[should_panic(expected = "no sizes within the buckets sum to")]
fn a_total_out_of_the_buckets_reach_is_refused() {
    // Ten files of at most 100 bytes cannot hold 1,001.
    plan(&Profile {
        bytes: 1_001,
        ..tiny()
    });
}

#[test]
#[should_panic(expected = "fans out to nothing")]
fn a_fan_out_of_zero_is_refused() {
    plan(&Profile {
        fan_out: 0,
        ..tiny()
    });
}

#[test]
#[should_panic(expected = "smallest above its largest")]
fn a_bucket_turned_inside_out_is_refused() {
    plan(&Profile {
        sizes: &[SizeBucket {
            permille: 1000,
            smallest: 101,
            largest: 100,
        }],
        ..tiny()
    });
}

#[test]
#[should_panic(expected = "overlap")]
fn overlapping_buckets_are_refused() {
    plan(&Profile {
        sizes: &[
            SizeBucket {
                permille: 500,
                smallest: 0,
                largest: 50,
            },
            SizeBucket {
                permille: 500,
                smallest: 50,
                largest: 100,
            },
        ],
        ..tiny()
    });
}

#[test]
fn a_files_content_has_exactly_its_planned_size_and_follows_from_its_seed() {
    let file = PlannedFile {
        path: "notes.md".to_owned(),
        size: 10_000,
        content_seed: 3,
    };
    let text = content(&SMALL, &file);

    assert_eq!(text.len(), 10_000);
    assert_eq!(text, content(&SMALL, &file));
    let reseeded = PlannedFile {
        content_seed: 4,
        ..file.clone()
    };
    assert_ne!(text, content(&SMALL, &reseeded));
    // Text, as a project holds it: printable ASCII in lines.
    assert!(
        text.iter()
            .all(|byte| byte.is_ascii_graphic() || *byte == b' ' || *byte == b'\n')
    );
    assert!(text.contains(&b'\n'));
}

#[test]
fn an_empty_file_has_no_content() {
    let file = PlannedFile {
        path: "empty.txt".to_owned(),
        size: 0,
        content_seed: 1,
    };

    assert!(content(&SMALL, &file).is_empty());
}

#[test]
fn materializing_a_plan_writes_every_planned_path_at_its_size() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let profile = tiny();
    let tree = plan(&profile);

    materialize(&profile, &tree, directory.path()).expect("the tree is written");

    for path in tree.directories.iter().chain(&tree.excluded_directories) {
        assert!(
            directory.path().join(path).is_dir(),
            "{path} is a directory"
        );
    }
    for file in tree.files.iter().chain(&tree.excluded_files) {
        let written = fs::read(directory.path().join(&file.path)).expect("a planned file");
        assert_eq!(written.len() as u64, file.size, "{}", file.path);
        assert_eq!(written, content(&profile, file), "{}", file.path);
    }
    let planned = tree.directories.len()
        + tree.excluded_directories.len()
        + tree.files.len()
        + tree.excluded_files.len();
    assert_eq!(
        entries_beneath(directory.path()),
        planned,
        "nothing beyond the plan"
    );
}

#[test]
fn the_profiles_policy_excludes_the_heavy_subtree_and_admits_every_planned_path() {
    let policy = SMALL.policy();
    let tree = plan(&SMALL);

    assert!(matches!(
        policy.directory(&rp("node_modules")),
        Verdict::Excluded(_)
    ));
    for directory in &tree.directories {
        assert!(
            matches!(policy.directory(&rp(directory)), Verdict::Captured),
            "{directory}"
        );
    }
    for file in &tree.files {
        assert!(
            matches!(policy.file(&rp(&file.path), file.size), Verdict::Captured),
            "{}",
            file.path
        );
    }
}

#[test]
fn the_small_fixtures_content_compresses_like_text() {
    // Measured as ticket 30 measured this project's Markdown — zlib at level 6 over the content as one
    // stream, 2.71× — because the figure is the fixture's own. What the store makes of it, one blob at a
    // time at the fastest level, is what the growth line measures.
    let tree = plan(&SMALL);
    let mut stream = ZlibEncoder::new(Vec::new(), Compression::new(6));
    for file in &tree.files {
        stream
            .write_all(&content(&SMALL, file))
            .expect("compressed in memory");
    }
    let compressed = stream.finish().expect("compressed in memory");

    let ratio = total(&tree) as f64 / compressed.len() as f64;
    assert_eq!(SMALL.compressibility_permille, 2_700);
    let target = 2.7;
    assert!(
        (target * 0.85..=target * 1.15).contains(&ratio),
        "{ratio:.2}× against a declared {target:.2}×"
    );
}

/// A profile small enough to reason about by hand: ten files of at most 100 bytes, totalling 500, in
/// three directories two wide.
fn tiny() -> Profile {
    Profile {
        name: "tiny",
        seed: 7,
        files: 10,
        directories: 3,
        bytes: 500,
        sizes: &[SizeBucket {
            permille: 1000,
            smallest: 0,
            largest: 100,
        }],
        fan_out: 2,
        excluded: Excluded {
            name: "cache",
            files: 3,
            directories: 1,
            file_size: 1,
        },
        ..SMALL
    }
}

/// Every directory and file beneath `directory`, at any depth.
fn entries_beneath(directory: &Path) -> usize {
    fs::read_dir(directory)
        .expect("a readable directory")
        .map(|entry| {
            let entry = entry.expect("a directory entry");
            if entry.file_type().expect("a file type").is_dir() {
                1 + entries_beneath(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

/// `path` as a Root-relative path.
fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

/// Every byte the plan's included files hold.
fn total(tree: &TreePlan) -> u64 {
    tree.files.iter().map(|file| file.size).sum()
}

/// How many of the plan's files each of the profile's buckets holds, in bucket order.
fn held_per_bucket(profile: &Profile, tree: &TreePlan) -> Vec<usize> {
    let mut held = vec![0; profile.sizes.len()];
    for file in &tree.files {
        let bucket = profile
            .sizes
            .iter()
            .position(|bucket| bucket.holds(file.size))
            .unwrap_or_else(|| panic!("{}: {} bytes is in no bucket", profile.name, file.size));
        held[bucket] += 1;
    }
    held
}

#[test]
fn a_size_bucket_holds_exactly_the_sizes_between_its_bounds() {
    let bucket = SizeBucket {
        permille: 1000,
        smallest: 10,
        largest: 20,
    };

    assert!(bucket.holds(10));
    assert!(bucket.holds(15));
    assert!(bucket.holds(20));
    assert!(!bucket.holds(9));
    assert!(!bucket.holds(21));
}

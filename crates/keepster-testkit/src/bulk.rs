//! The bulk generator (ticket 30; the second of ticket 31's three layers): a seeded tree whose shape is
//! declared rather than incidentally plausible.
//!
//! A [`Profile`] states every parameter the measured numbers depend on — how many files and directories,
//! how many bytes, how sizes are distributed, how wide a directory fans out, and a heavy subtree the
//! Include Rules exclude. [`plan`] turns a profile into a [`TreePlan`], a pure value naming every path and
//! its size, so the ceiling profile can be checked without writing ten gibibytes; writing a plan to disk is
//! a separate step.
//!
//! The thresholds the tripwire gates on are calibrated to these shapes, and the baselines it records are
//! comparable from run to run only on them, which is why the profiles are frozen and the gate checks every
//! digest, a baseline's fixture's included: moving one moves every measured number (ticket 31). Nothing here is shared with the degenerate
//! catalogue, whose cases must be free to grow.

use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use keepster_core::{CapturePolicy, DEFAULT_MAX_SIZE, Level, RootPath, Rule, RuleOrigin, RuleSet};
use rand::rngs::ChaCha8Rng;
use rand::{Rng, SeedableRng};

/// The declared shape of a generated Root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// How the profile is named in reports and in the thresholds file.
    pub name: &'static str,
    /// The seed every random choice derives from: one seed, one tree.
    pub seed: u64,
    /// Included regular files, exactly.
    pub files: usize,
    /// Included directories beneath the Root, exactly; the Root itself is not counted.
    pub directories: usize,
    /// The included files' sizes summed, exactly.
    pub bytes: u64,
    /// How file sizes are distributed. The shares sum to 1000‰, and `bytes` must lie between what every
    /// file at its bucket's smallest and every file at its bucket's largest would sum to.
    pub sizes: &'static [SizeBucket],
    /// The most included subdirectories any directory holds — what the watch budget is spent on (ticket
    /// 02). The excluded directory sits at the Root beside them and is not counted: it is never watched.
    pub fan_out: usize,
    /// The heavy subtree the Include Rules keep out.
    pub excluded: Excluded,
    /// How well the included files compress, in thousandths: their content's bytes over its zlib
    /// compression at level 6 as one stream — exactly how ticket 30 measured this project's own Markdown
    /// at 2.71×. Random bytes would not compress at all and would overstate store growth by that factor.
    /// The figure belongs to the fixture, not to the store, which deflates each blob alone at the fastest
    /// level; what the store makes of it is the growth line's to measure. A test holds the generated
    /// content to this figure; generation itself only follows `noise_permille`.
    pub compressibility_permille: u32,
    /// The knob that reaches `compressibility_permille`: the share of words, in thousandths, that are
    /// random characters rather than vocabulary — identifiers, hashes and numbers, in a real tree.
    pub noise_permille: u32,
    /// How the tree changes over time, for the outcome lines that exist only over time: store growth
    /// and coverage (ticket 30).
    pub churn: Churn,
}

/// The declared change stream: `steps` steps, each the changes one capture sees, `interval` apart —
/// so a churn is also a rate, the files per hour ticket 30 asks for.
///
/// Every changed or created file takes a size from the profile's buckets — a changed file stays in its
/// bucket, a created one lands in a bucket by its share — and wholly new content, so each step costs the
/// store a new blob per file it touches.
///
/// Those sizes are drawn uniformly within the bucket, while a planned size has been fitted toward its
/// bucket's lower edge (see [`plan`]), so the live tree grows a little as the stream runs: by 4.9 % of
/// `bytes` over the small profile's stream and 2.3 % over the ceiling's. A measurement over the stream
/// takes the tree's size at its end, never `bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Churn {
    /// Steps in the stream.
    pub steps: usize,
    /// The time between two steps: what a step is dated by, since history over time — the Retention
    /// ladder above all — depends on when, not only on what.
    pub interval: Duration,
    /// Existing files rewritten per step.
    pub modified: usize,
    /// New files per step.
    pub created: usize,
    /// Existing files removed per step.
    pub deleted: usize,
}

impl Profile {
    /// The capture policy the fixture is measured under: the built-in defaults plus the one Include
    /// Rule that excludes [`Excluded::name`].
    pub fn policy(&self) -> CapturePolicy {
        let rule = Rule::parse(
            &format!("{}/", self.excluded.name),
            RootPath::root(),
            RuleOrigin {
                level: Level::Global,
                source: format!("the {} bench profile", self.name),
                line: None,
            },
        )
        .expect("a directory name is a valid pattern");
        CapturePolicy::new(
            RuleSet::new(vec![rule]),
            RuleSet::default(),
            DEFAULT_MAX_SIZE,
            Vec::new(),
        )
    }
}

/// A share of the files, each sized within a closed range of bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeBucket {
    /// The share of the files in this bucket, in thousandths.
    pub permille: u32,
    /// The smallest size in this bucket, in bytes.
    pub smallest: u64,
    /// The largest size in this bucket, in bytes.
    pub largest: u64,
}

impl SizeBucket {
    /// Whether a file of `size` bytes is in this bucket: its bounds are both held.
    pub fn holds(&self, size: u64) -> bool {
        (self.smallest..=self.largest).contains(&size)
    }
}

/// A directory at the Root, in the shape of a dependency cache, that the profile's Include Rule excludes.
///
/// It exists so the lines that defend "an excluded directory is never entered" (ADR 0007) and "rules are
/// applied before watches are installed" (ticket 02) have something to not enter: without it, breaking
/// either invariant would cost nothing measurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Excluded {
    /// The directory's name at the Root, which is also the Include Rule that excludes it, as `name/`.
    pub name: &'static str,
    /// Regular files inside it, at any depth.
    pub files: usize,
    /// Directories beneath it; the excluded directory itself is not counted.
    pub directories: usize,
    /// The size of every file inside it, in bytes.
    pub file_size: u64,
}

/// The tripwire's fixture: generated and measured in seconds, because a gate that takes ten minutes is
/// never run by hand (ticket 30).
pub const SMALL: Profile = Profile {
    name: "small",
    seed: 0x6b65_6570_7374_6572,
    files: 5_000,
    directories: 500,
    bytes: 50 * MIB,
    sizes: &[
        SizeBucket {
            permille: 500,
            smallest: 0,
            largest: 2 * KIB,
        },
        SizeBucket {
            permille: 350,
            smallest: 2 * KIB + 1,
            largest: 16 * KIB,
        },
        SizeBucket {
            permille: 130,
            smallest: 16 * KIB + 1,
            largest: 64 * KIB,
        },
        SizeBucket {
            permille: 20,
            smallest: 64 * KIB + 1,
            largest: 256 * KIB,
        },
    ],
    fan_out: 8,
    excluded: Excluded {
        name: "node_modules",
        files: 20_000,
        directories: 2_000,
        file_size: KIB,
    },
    compressibility_permille: 2_700,
    noise_permille: NOISE,
    churn: Churn {
        steps: 50,
        interval: STEP,
        modified: 20,
        created: 2,
        deleted: 1,
    },
};

/// The outcome claim's fixture: v1's per-Root ceiling of 100,000 files, 10,000 directories and 10 GiB
/// (ticket 30), run at release.
pub const CEILING: Profile = Profile {
    name: "ceiling",
    seed: 0x6365_696c_696e_6721,
    files: 100_000,
    directories: 10_000,
    bytes: 10 * GIB,
    sizes: &[
        SizeBucket {
            permille: 450,
            smallest: 0,
            largest: 4 * KIB,
        },
        SizeBucket {
            permille: 300,
            smallest: 4 * KIB + 1,
            largest: 64 * KIB,
        },
        SizeBucket {
            permille: 200,
            smallest: 64 * KIB + 1,
            largest: 512 * KIB,
        },
        SizeBucket {
            permille: 45,
            smallest: 512 * KIB + 1,
            largest: 2 * MIB,
        },
        SizeBucket {
            permille: 5,
            smallest: 2 * MIB + 1,
            largest: 8 * MIB,
        },
    ],
    fan_out: 10,
    excluded: Excluded {
        name: "node_modules",
        files: 100_000,
        directories: 10_000,
        file_size: KIB,
    },
    compressibility_permille: 2_700,
    noise_permille: NOISE,
    churn: Churn {
        steps: 50,
        interval: STEP,
        modified: 100,
        created: 10,
        deleted: 5,
    },
};

/// The fixture of the line measured for "entries are applied in lexicographic order" (ticket 12): every
/// file in one directory, where an insertion out of order costs the most. At 20k files a broken order
/// costs too small a part of a first capture to breach a limit, so the line is a baseline recording the
/// capture; doubling it would take some 800k files (see the line's reason). Every file is empty, so the
/// store holds one blob for all of them and a first capture costs the walk and the index — what a broken
/// order multiplies — rather than writing blobs, which would drown it further. Its excluded directory, which every profile has, holds one file
/// and nothing else: the line is not about exclusion, and an empty directory would be a degenerate case.
pub const FLAT: Profile = Profile {
    name: "flat",
    seed: 0x666c_6174_7472_6565,
    files: 20_000,
    directories: 0,
    bytes: 0,
    sizes: &[SizeBucket {
        permille: 1000,
        smallest: 0,
        largest: 0,
    }],
    fan_out: 1,
    excluded: Excluded {
        name: "node_modules",
        files: 1,
        directories: 0,
        file_size: KIB,
    },
    compressibility_permille: 2_700,
    noise_permille: NOISE,
    churn: Churn {
        steps: 0,
        interval: STEP,
        modified: 0,
        created: 0,
        deleted: 0,
    },
};

/// The fixture of the lines defending "an excluded directory is never entered" (ADR 0007) and "Include
/// Rules are applied before watches are installed" (ticket 02): a small included tree beside a large
/// excluded one, so entering the excluded one would cost many times what the included one does.
pub const EXCLUSION: Profile = Profile {
    name: "exclusion",
    seed: 0x6578_636c_7564_6564,
    files: 100,
    directories: 10,
    bytes: 100 * KIB,
    sizes: &[SizeBucket {
        permille: 1000,
        smallest: 0,
        largest: 2 * KIB,
    }],
    fan_out: 8,
    excluded: Excluded {
        name: "node_modules",
        files: 20_000,
        directories: 2_000,
        file_size: KIB,
    },
    compressibility_permille: 2_700,
    noise_permille: NOISE,
    churn: Churn {
        steps: 0,
        interval: STEP,
        modified: 0,
        created: 0,
        deleted: 0,
    },
};

/// The profiles the tripwire runs, each with how its [`digest`] treats content: every profile's is
/// hashed but the ceiling's, ten gibibytes (ticket 31). The thresholds file holds a digest for
/// exactly these, and the runner reads the choice from here rather than making it again.
pub const BENCH_PROFILES: &[(Profile, Contents)] = &[
    (SMALL, Contents::Hashed),
    (CEILING, Contents::Omitted),
    (FLAT, Contents::Hashed),
    (EXCLUSION, Contents::Hashed),
];

/// The share of random words that brings the vocabulary's text to about 2.7× (see
/// [`Profile::noise_permille`]), found by measurement: zlib at level 6 over the small profile's content
/// gave 3.08× with none, 2.79× at 25‰, 2.70× at 35‰ and 2.59× at 50‰.
const NOISE: u32 = 35;

/// One step a minute: ticket 04's rate limit, the most often a Root is ever captured, so the declared
/// churn is the heaviest a capture can be asked to keep up with.
const STEP: Duration = Duration::from_secs(60);

/// A kibibyte, in bytes.
const KIB: u64 = 1024;
/// A mebibyte, in bytes.
const MIB: u64 = 1024 * KIB;
/// A gibibyte, in bytes.
const GIB: u64 = 1024 * MIB;

/// Every path a profile generates, Root-relative and `/`-joined, with nothing yet on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreePlan {
    /// Included directories, each listed after its parent.
    pub directories: Vec<String>,
    /// Included files.
    pub files: Vec<PlannedFile>,
    /// The excluded directory and every directory beneath it, each listed after its parent.
    pub excluded_directories: Vec<String>,
    /// Files inside the excluded directory.
    pub excluded_files: Vec<PlannedFile>,
}

/// One file of a plan: where, how large, and the seed its content is generated from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Root-relative and `/`-joined.
    pub path: String,
    /// In bytes.
    pub size: u64,
    /// Seeds this file's content, so a file can be written again identically without the whole plan.
    pub content_seed: u64,
}

/// The tree `profile` declares. The same profile always yields the same plan, on every platform.
///
/// No planned directory is empty: an empty directory is a degenerate case of its own (ticket 25), which
/// belongs in the catalogue rather than inside a fixture measured without declaring it.
///
/// # Panics
///
/// When the profile contradicts itself — shares not summing to 1000‰, buckets inside out, overlapping
/// or out of order, a fan-out of zero, `bytes` out of the buckets' reach, or fewer files than directories
/// to put them in. Profiles are constants, so this is a mistake in a profile, not a condition to handle.
pub fn plan(profile: &Profile) -> TreePlan {
    check(profile);
    let mut draw = Draw::new(profile.seed);

    let directories = directory_tree("", profile.directories, profile.fan_out, &mut draw);
    let sizes = sizes(profile, &mut draw);
    // The Root is a place for files too, as it is in any real project.
    let files = place_files(profile, "", &directories, &sizes, &mut draw);

    let excluded = profile.excluded;
    let mut excluded_directories = vec![excluded.name.to_owned()];
    excluded_directories.extend(directory_tree(
        excluded.name,
        excluded.directories,
        profile.fan_out,
        &mut draw,
    ));
    let excluded_sizes = vec![excluded.file_size; excluded.files];
    let excluded_files = place_files(
        profile,
        excluded.name,
        &excluded_directories[1..],
        &excluded_sizes,
        &mut draw,
    );

    TreePlan {
        directories,
        files,
        excluded_directories,
        excluded_files,
    }
}

/// What `file` holds: text in lines of vocabulary words and random ones, exactly `file.size` bytes long,
/// the same for the same seed.
///
/// Text rather than random bytes because the store deflates every blob: the content decides how much
/// history costs, which is one of the outcome lines (ticket 30).
pub fn content(profile: &Profile, file: &PlannedFile) -> Vec<u8> {
    let size = usize::try_from(file.size).expect("a planned file fits in memory");
    let mut draw = Draw::new(file.content_seed);
    // The last word may run past `size` before the cut: by at most a random word's 12 bytes and a
    // separator, more than any vocabulary word.
    let mut text = Vec::with_capacity(size + 13);
    let mut line = draw.between(4, 14);
    while text.len() < size {
        if draw.below(1000) < profile.noise_permille as usize {
            let length = draw.between(3, 12);
            for _ in 0..length {
                text.push(ALPHABET[draw.below(ALPHABET.len())]);
            }
        } else {
            text.extend_from_slice(VOCABULARY[draw.below(VOCABULARY.len())].as_bytes());
        }
        line -= 1;
        if line == 0 {
            text.push(b'\n');
            line = draw.between(4, 14);
        } else {
            text.push(b' ');
        }
    }
    text.truncate(size);
    text
}

/// Write `tree` beneath `root`: every directory, then every file with its [`content`].
///
/// `root` may exist, but must hold none of the plan's paths: a fixture is written once, from nothing,
/// so that what is measured is exactly what was planned.
///
/// # Errors
///
/// The first write the filesystem refuses, a planned path already present among them; `root` may then
/// hold part of the tree.
pub fn materialize(profile: &Profile, tree: &TreePlan, root: &Path) -> io::Result<()> {
    fs::create_dir_all(root)?;
    // Listed after their parents, so each has somewhere to go.
    for directory in tree.directories.iter().chain(&tree.excluded_directories) {
        fs::create_dir(root.join(directory))?;
    }
    for file in tree.files.iter().chain(&tree.excluded_files) {
        // Created, never truncated: a file already there is refused rather than silently replaced.
        File::create_new(root.join(&file.path))?.write_all(&content(profile, file))?;
    }
    Ok(())
}

/// One change of a [`change_stream`] step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// An existing file rewritten: its new size and content seed.
    Modified(PlannedFile),
    /// A new file in the Root or one of the planned directories.
    Created(PlannedFile),
    /// An existing file removed, by its Root-relative path.
    Deleted(String),
}

/// The changes `profile`'s churn makes to `tree`, step by step; the same profile and tree always yield
/// the same stream.
///
/// Each change names a file as it stands at that moment, a step touches a path at most once — a file
/// changed twice between captures is one change to them — and no deletion leaves a directory empty,
/// which would make the stream's tree a degenerate case (ticket 25). The stream draws from a seed of
/// its own, so changing the churn never moves the tree.
///
/// # Panics
///
/// When the churn asks for more than the tree can give: more changes in a step than there are files to
/// change, or deletions with no file left whose directory would stay occupied.
pub fn change_stream(profile: &Profile, tree: &TreePlan) -> Vec<Vec<Change>> {
    let mut draw = Draw::new(profile.seed ^ STREAM);
    let mut live = tree.files.clone();
    let mut held: HashMap<String, usize> = HashMap::new();
    for file in &live {
        *held.entry(parent_of(&file.path).to_owned()).or_default() += 1;
    }
    let branching: HashSet<&str> = tree
        .directories
        .iter()
        .map(|directory| parent_of(directory))
        .collect();
    let mut homes: Vec<&str> = vec![""];
    homes.extend(tree.directories.iter().map(String::as_str));
    let mut created = 0;

    let churn = profile.churn;
    let mut stream = Vec::with_capacity(churn.steps);
    for _ in 0..churn.steps {
        let mut touched: HashSet<String> = HashSet::new();
        let mut step = Vec::with_capacity(churn.modified + churn.created + churn.deleted);

        for _ in 0..churn.modified {
            let index = pick(profile, &live, &mut draw, |file| {
                !touched.contains(&file.path)
            });
            let file = &mut live[index];
            file.size = resized(profile, file.size, &mut draw);
            file.content_seed = draw.next();
            touched.insert(file.path.clone());
            step.push(Change::Modified(file.clone()));
        }
        for _ in 0..churn.created {
            let home = homes[draw.below(homes.len())];
            // Numbered after every planned file, so no name is taken twice.
            let file = PlannedFile {
                path: join(home, &file_name(tree.files.len() + created, &mut draw)),
                size: fresh_size(profile, &mut draw),
                content_seed: draw.next(),
            };
            created += 1;
            *held.entry(home.to_owned()).or_default() += 1;
            touched.insert(file.path.clone());
            live.push(file.clone());
            step.push(Change::Created(file));
        }
        for _ in 0..churn.deleted {
            let index = pick(profile, &live, &mut draw, |file| {
                let home = parent_of(&file.path);
                !touched.contains(&file.path) && (held[home] > 1 || branching.contains(home))
            });
            let file = live.swap_remove(index);
            *held
                .get_mut(parent_of(&file.path))
                .expect("every live file's directory is counted") -= 1;
            touched.insert(file.path.clone());
            step.push(Change::Deleted(file.path));
        }
        stream.push(step);
    }
    stream
}

/// Write one step of a [`change_stream`] beneath `root`, where the tree and every earlier step already
/// are.
///
/// # Errors
///
/// The first write or removal the filesystem refuses; the step may then be partly applied.
pub fn apply(profile: &Profile, step: &[Change], root: &Path) -> io::Result<()> {
    for change in step {
        match change {
            Change::Modified(file) | Change::Created(file) => {
                fs::write(root.join(&file.path), content(profile, file))?;
            }
            Change::Deleted(path) => fs::remove_file(root.join(path))?,
        }
    }
    Ok(())
}

/// Mixed into a profile's seed for its change stream, so the stream and the tree draw independently.
const STREAM: u64 = 0x7374_7265_616d_0000;

/// The index of a file in `live` that `allowed` accepts, drawn uniformly by rejection: a pick is cheap
/// and nearly every file qualifies, where filtering the whole list per pick would cost the ceiling's
/// hundred thousand files every time.
fn pick(
    profile: &Profile,
    live: &[PlannedFile],
    draw: &mut Draw,
    allowed: impl Fn(&PlannedFile) -> bool,
) -> usize {
    for _ in 0..REJECTIONS {
        let index = draw.below(live.len());
        if allowed(&live[index]) {
            return index;
        }
    }
    panic!(
        "{}: the churn asks for a change no file can take",
        profile.name
    );
}

/// How many draws [`pick`] makes before deciding no file qualifies. A profile that comes anywhere near
/// it is asking for most of its files in one step, which no profile here does.
const REJECTIONS: usize = 10_000;

/// A new size for a file of `size` bytes, from the bucket it is in.
fn resized(profile: &Profile, size: u64, draw: &mut Draw) -> u64 {
    let bucket = profile
        .sizes
        .iter()
        .find(|bucket| bucket.holds(size))
        .expect("every planned size lies in a bucket");
    draw.between(bucket.smallest, bucket.largest)
}

/// A size for a new file: a bucket chosen by its share, then a size within it.
fn fresh_size(profile: &Profile, draw: &mut Draw) -> u64 {
    let mut point = u32::try_from(draw.below(1000)).expect("below a thousand");
    for bucket in profile.sizes {
        if point < bucket.permille {
            return draw.between(bucket.smallest, bucket.largest);
        }
        point -= bucket.permille;
    }
    unreachable!("the shares sum to 1000‰, checked with the profile")
}

/// The directory holding `path`, `""` being the Root.
fn parent_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Whether a [`digest`] hashes what each included file holds, or only where and how large it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contents {
    /// Every included file's content is generated and hashed: a change to how content is written — the
    /// vocabulary, the alphabet, the line lengths — moves the digest though no parameter moved.
    Hashed,
    /// Paths, sizes and seeds only, for a profile whose content is too large to generate for a test; the
    /// small profile's digest already covers the generator they share.
    Omitted,
}

/// A fingerprint of everything `profile` makes: the git blob id of its [`canonical`] text.
///
/// The thresholds file records the digest each profile had when its thresholds were calibrated, and a
/// test compares it with this one (ticket 31), so neither a parameter nor the generator behind it can
/// move without the thresholds moving in the same commit.
pub fn digest(profile: &Profile, contents: Contents) -> String {
    blob_id(canonical(profile, contents).as_bytes())
}

/// Everything `profile` makes, as text: its parameters, every path and size of its tree and of its
/// change stream, and — with [`Contents::Hashed`] — every included file's content as a blob id.
///
/// One fact a line, fields split by spaces; [`plan`]'s check keeps whitespace out of the names, so two
/// profiles cannot write the same text. It is written field by field rather than through `Debug`, whose
/// format is not promised to stay put. The excluded files' content is never hashed: a capture never
/// reads it.
pub fn canonical(profile: &Profile, contents: Contents) -> String {
    let mut text = String::new();
    write_canonical(&mut text, profile, contents).expect("writing to a `String` cannot fail");
    text
}

/// Write [`canonical`]'s text for `profile` to `text`.
fn write_canonical(text: &mut String, profile: &Profile, contents: Contents) -> fmt::Result {
    let tree = plan(profile);
    writeln!(
        text,
        "profile {}\nseed {}\nfiles {}\ndirectories {}\nbytes {}\nfan-out {}\n\
         compressibility {}\nnoise {}\ncontents {}",
        profile.name,
        profile.seed,
        profile.files,
        profile.directories,
        profile.bytes,
        profile.fan_out,
        profile.compressibility_permille,
        profile.noise_permille,
        match contents {
            Contents::Hashed => "hashed",
            Contents::Omitted => "omitted",
        },
    )?;
    for bucket in profile.sizes {
        writeln!(
            text,
            "bucket {} {} {}",
            bucket.permille, bucket.smallest, bucket.largest
        )?;
    }
    let Excluded {
        name,
        files,
        directories,
        file_size,
    } = profile.excluded;
    writeln!(text, "excluded {name} {files} {directories} {file_size}")?;
    let Churn {
        steps,
        interval,
        modified,
        created,
        deleted,
    } = profile.churn;
    writeln!(
        text,
        "churn {steps} {} {modified} {created} {deleted}",
        interval.as_nanos()
    )?;

    for directory in &tree.directories {
        writeln!(text, "directory {directory}")?;
    }
    for file in &tree.files {
        write!(
            text,
            "file {} {} {}",
            file.path, file.size, file.content_seed
        )?;
        if contents == Contents::Hashed {
            write!(text, " {}", blob_id(&content(profile, file)))?;
        }
        text.push('\n');
    }
    for directory in &tree.excluded_directories {
        writeln!(text, "excluded-directory {directory}")?;
    }
    for file in &tree.excluded_files {
        writeln!(
            text,
            "excluded-file {} {} {}",
            file.path, file.size, file.content_seed
        )?;
    }
    for (number, step) in change_stream(profile, &tree).iter().enumerate() {
        for change in step {
            match change {
                Change::Modified(file) => writeln!(
                    text,
                    "step {number} modified {} {} {}",
                    file.path, file.size, file.content_seed
                ),
                Change::Created(file) => writeln!(
                    text,
                    "step {number} created {} {} {}",
                    file.path, file.size, file.content_seed
                ),
                Change::Deleted(path) => writeln!(text, "step {number} deleted {path}"),
            }?;
        }
    }
    Ok(())
}

/// The git blob id of `bytes`, in hexadecimal.
fn blob_id(bytes: &[u8]) -> String {
    git2::Oid::hash_object(git2::ObjectType::Blob, bytes)
        .expect("hashing bytes in memory cannot fail")
        .to_string()
}

/// Refuse a profile that contradicts itself, before anything is drawn from it.
fn check(profile: &Profile) {
    let name = profile.name;
    // The canonical text splits fields on spaces and facts on line breaks; a name holding either could
    // make two profiles write the same text.
    for (what, given) in [
        ("profile", name),
        ("excluded directory", profile.excluded.name),
    ] {
        assert!(!given.is_empty(), "{name}: the {what} needs a name");
        assert!(
            !given
                .chars()
                .any(|character| character.is_whitespace() || character.is_control()),
            "{name}: the {what} name {given:?} holds whitespace or a control character"
        );
    }
    let shares: u32 = profile.sizes.iter().map(|bucket| bucket.permille).sum();
    assert_eq!(shares, 1000, "{name}: the size shares must sum to 1000‰");
    assert!(
        profile.fan_out > 0,
        "{name}: a directory tree that fans out to nothing"
    );
    for bucket in profile.sizes {
        assert!(
            bucket.smallest <= bucket.largest,
            "{name}: a bucket with its smallest above its largest"
        );
    }
    // Ascending and disjoint, so every size names exactly one bucket.
    for pair in profile.sizes.windows(2) {
        assert!(
            pair[0].largest < pair[1].smallest,
            "{name}: size buckets overlap or are out of order"
        );
    }
}

/// `count` directories beneath `base`, laid out as a complete tree in which no directory holds more than
/// `fan_out` subdirectories: the n-th directory's parent is the ((n − 1) / `fan_out`)-th, `base` being the
/// 0th. The layout is deterministic, so only the names draw from the seed.
fn directory_tree(base: &str, count: usize, fan_out: usize, draw: &mut Draw) -> Vec<String> {
    let mut paths: Vec<String> = Vec::with_capacity(count);
    for number in 1..=count {
        let parent = (number - 1) / fan_out;
        let parent = if parent == 0 {
            base
        } else {
            &paths[parent - 1]
        };
        let name = format!("{}-{number}", draw.word());
        paths.push(join(parent, &name));
    }
    paths
}

/// One file per size, beneath `base` or one of `directories` (each listed after its parent): the first
/// files go one to each directory with no subdirectory, so none is left empty, and the rest anywhere.
fn place_files(
    profile: &Profile,
    base: &str,
    directories: &[String],
    sizes: &[u64],
    draw: &mut Draw,
) -> Vec<PlannedFile> {
    let parents: HashSet<&str> = directories
        .iter()
        .map(|directory| parent_of(directory))
        .collect();
    let mut leaves: Vec<&str> = directories
        .iter()
        .map(String::as_str)
        .filter(|directory| !parents.contains(directory))
        .collect();
    if directories.is_empty() {
        leaves.push(base);
    }
    assert!(
        sizes.len() >= leaves.len(),
        "{}: {} files cannot fill {} directories with no subdirectory",
        profile.name,
        sizes.len(),
        leaves.len()
    );

    let mut homes: Vec<&str> = vec![base];
    homes.extend(directories.iter().map(String::as_str));
    sizes
        .iter()
        .enumerate()
        .map(|(number, &size)| {
            let home = match leaves.get(number) {
                Some(leaf) => leaf,
                None => homes[draw.below(homes.len())],
            };
            PlannedFile {
                path: join(home, &file_name(number, draw)),
                size,
                content_seed: draw.next(),
            }
        })
        .collect()
}

/// One size per file: each bucket's share drawn uniformly within its range, then fitted so the sizes sum
/// to exactly `bytes` without leaving any bucket, and finally shuffled so a file's number says nothing
/// about its size.
///
/// The declared shape is therefore the buckets' counts and the total. Within a bucket, fitting moves
/// every size in proportion to its room, so when the draws overshoot `bytes` the sizes gather toward
/// each bucket's lower edge, and toward its upper edge when they fall short.
fn sizes(profile: &Profile, draw: &mut Draw) -> Vec<u64> {
    let mut counts: Vec<usize> = profile
        .sizes
        .iter()
        .map(|bucket| profile.files * bucket.permille as usize / 1000)
        .collect();
    // What rounding down left over goes to the first bucket, so the count is exact.
    counts[0] += profile.files - counts.iter().sum::<usize>();

    let mut drawn: Vec<Drawn> = Vec::with_capacity(profile.files);
    for (bucket, count) in profile.sizes.iter().zip(counts) {
        for _ in 0..count {
            drawn.push(Drawn {
                size: draw.between(bucket.smallest, bucket.largest),
                bucket: *bucket,
            });
        }
    }
    fit(&mut drawn, profile.bytes, profile.name);
    draw.shuffle(&mut drawn);

    drawn.into_iter().map(|drawn| drawn.size).collect()
}

/// A drawn size and the bucket it must stay in.
struct Drawn {
    /// The size, in bytes.
    size: u64,
    /// The bucket it was drawn from.
    bucket: SizeBucket,
}

impl Drawn {
    /// How far this size can move toward the total without leaving its bucket.
    fn room(&self, growing: bool) -> u64 {
        if growing {
            self.bucket.largest - self.size
        } else {
            self.size - self.bucket.smallest
        }
    }

    /// Move this size `by` bytes toward the total; `by` is never more than its room.
    fn shift(&mut self, by: u64, growing: bool) {
        if growing {
            self.size += by;
        } else {
            self.size -= by;
        }
    }
}

/// Move every size toward `total` in proportion to its room, so the sum lands on `total` exactly and no
/// size leaves its bucket.
fn fit(drawn: &mut [Drawn], total: u64, profile: &str) {
    let sum: u64 = drawn.iter().map(|drawn| drawn.size).sum();
    let growing = sum < total;
    let mut needed = sum.abs_diff(total);
    let capacity: u64 = drawn.iter().map(|drawn| drawn.room(growing)).sum();
    assert!(
        needed <= capacity,
        "{profile}: no sizes within the buckets sum to {total} bytes"
    );
    if needed == 0 {
        return;
    }

    let mut moved = 0;
    for entry in drawn.iter_mut() {
        // At most `room`, since `needed <= capacity`; the product fits in 128 bits.
        let share = u128::from(needed) * u128::from(entry.room(growing)) / u128::from(capacity);
        let share = u64::try_from(share).expect("a share of what is needed is no larger than it");
        entry.shift(share, growing);
        moved += share;
    }
    // Rounding down left less than one byte per size. It goes a byte at a time wherever room remains,
    // and since the room left is at least what is still needed, every round moves something.
    needed -= moved;
    while needed > 0 {
        for entry in drawn.iter_mut() {
            if needed > 0 && entry.room(growing) > 0 {
                entry.shift(1, growing);
                needed -= 1;
            }
        }
    }
}

/// A name for the `number`-th file: a word for plausibility, the number for uniqueness, and an extension.
fn file_name(number: usize, draw: &mut Draw) -> String {
    let extension = EXTENSIONS[draw.below(EXTENSIONS.len())];
    format!("{}-{number}.{extension}", draw.word())
}

/// `name` inside `directory`, `""` being the Root.
fn join(directory: &str, name: &str) -> String {
    if directory.is_empty() {
        name.to_owned()
    } else {
        format!("{directory}/{name}")
    }
}

/// Words names are made from. Changing this list changes every generated path.
const WORDS: &[&str] = &[
    "alpha", "anchor", "basin", "beacon", "cedar", "cinder", "delta", "ember", "falcon", "garnet",
    "harbor", "indigo", "juniper", "kestrel", "lantern", "meadow", "nectar", "orbit", "pepper",
    "quartz", "raven", "saffron", "timber", "umber", "velvet", "willow", "yarrow", "zephyr",
];

/// The words content is written in, beside the random ones. Changing this list changes every file.
const VOCABULARY: &[&str] = &[
    "the", "a", "of", "to", "and", "in", "is", "it", "for", "on", "with", "as", "be", "this",
    "that", "not", "or", "by", "from", "at", "if", "else", "return", "let", "fn", "pub", "struct",
    "impl", "match", "self", "use", "mod", "const", "type", "where", "while", "loop", "break",
    "true", "false", "None", "Some", "Ok", "Err", "String", "Vec", "Option", "Result", "path",
    "file", "root", "snapshot", "capture", "store", "index", "tree", "entry", "value", "error",
    "name", "size", "time", "change", "version", "history", "read", "write", "open", "close",
    "list", "count", "first", "last", "next", "previous", "when", "then", "every", "each", "which",
    "what", "only", "never", "always", "because", "before", "after", "between", "within",
    "without", "here", "there", "one", "two", "three", "#", "##", "-", "*", "=", "==", "=>", "->",
    "{", "}", "(", ")", "[", "]", ";", ",", ".", ":", "//", "///",
];

/// The characters random words are made of.
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// Extensions file names end in. Changing this list changes every generated path.
const EXTENSIONS: &[&str] = &["rs", "md", "txt", "json", "toml", "ts"];

/// The seeded source of every choice.
///
/// Only [`Rng::next_u64`] is used, and ranges are mapped here, because ChaCha8's stream is a part of
/// `rand` documented as value-stable; a distribution's algorithm may change between releases, which
/// would move every fixture without any parameter moving.
struct Draw(ChaCha8Rng);

impl Draw {
    /// The source for `seed`.
    fn new(seed: u64) -> Self {
        Self(ChaCha8Rng::seed_from_u64(seed))
    }

    /// The next 64 bits of the stream.
    fn next(&mut self) -> u64 {
        self.0.next_u64()
    }

    /// A number in `0..bound` by multiply-and-shift: the bias is below `bound / 2^64`, nothing a fixture's
    /// shape can show.
    fn below(&mut self, bound: usize) -> usize {
        let bound = bound as u64;
        let scaled = (u128::from(self.next()) * u128::from(bound)) >> 64;
        usize::try_from(scaled).expect("below a bound that is itself a usize")
    }

    /// A number in `smallest..=largest`.
    fn between(&mut self, smallest: u64, largest: u64) -> u64 {
        let span = u128::from(largest - smallest) + 1;
        let offset = (u128::from(self.next()) * span) >> 64;
        smallest + u64::try_from(offset).expect("below a span of at most 2^64")
    }

    /// One of [`WORDS`].
    fn word(&mut self) -> &'static str {
        WORDS[self.below(WORDS.len())]
    }

    /// Fisher–Yates.
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for last in (1..items.len()).rev() {
            let other = self.below(last + 1);
            items.swap(last, other);
        }
    }
}

//! What a capture does with one path: the user's Include Rules, the built-in secret denylist and its
//! opt-in list, the size cap, and the names no rule may admit.
//!
//! The order of the questions is the order of the answers `explain` gives: a reserved name is refused
//! before anything else is consulted; the user's rules come next, because an exclude rule is the user's
//! own word; the denylist then refuses what looks like a secret unless the user opted that path in; and
//! the size cap is asked last, because a file over it is Withheld — recorded as present — rather than
//! left out.

use std::path::{Path, PathBuf};

use crate::path::RootPath;
use crate::rules::{Kind, Level, Rule, RuleOrigin, RuleSet};
use crate::sidecar::{SIDECAR, WithheldReason};

/// The size cap a user who configures nothing gets: 100 MiB (ticket 10).
///
/// It bounds the daemon's memory as well as the disk, because libgit2 inflates a packed object whole to
/// read any of it.
pub const DEFAULT_MAX_SIZE: u64 = 100 * 1024 * 1024;

/// The names the built-in denylist refuses by default.
///
/// Names, not contents: a pattern without a `/` matches at any depth, so `keys/id_rsa` is refused as
/// surely as `id_rsa`. The list is deliberately short and recognisable — environment files, private
/// keys and key stores, and the credential files tools write into a project — because every entry is a
/// file some user will one day wonder about, and `explain` names the entry that refused it.
const SECRET_NAMES: &[&str] = &[
    ".env*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.keystore",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    ".netrc",
    ".pgpass",
    "local.properties",
];

/// Where the denylist's rules say they came from.
const SECRET_SOURCE: &str = "the built-in secret denylist";

/// Why a size was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a size: write a whole number with an optional unit, such as `100MiB` or `500KB`"
)]
pub struct InvalidSize(pub String);

/// Read a size such as `100MiB`, `1 GiB`, `500KB` or `42`.
///
/// Binary units (`KiB`, `MiB`, `GiB`, `TiB`) are powers of 1024 and decimal ones (`KB`, `MB`, `GB`,
/// `TB`) powers of 1000, as their names say; a bare number is bytes. Only whole numbers: a fractional
/// size would be rounded somewhere, and the cap is a promise about which file is kept.
pub fn parse_size(text: &str) -> Result<u64, InvalidSize> {
    let invalid = || InvalidSize(text.to_owned());

    let trimmed = text.trim();
    let digits = trimmed
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(digits);
    let number: u64 = number.parse().map_err(|_| invalid())?;

    let multiplier: u64 = match unit.trim() {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        _ => return Err(invalid()),
    };

    number.checked_mul(multiplier).ok_or_else(invalid)
}

/// One per-pattern size cap: the last matching one replaces the scalar for the paths it matches, and a
/// negated one returns them to the scalar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeRule {
    /// Which paths it applies to.
    pub rule: Rule,
    /// Their cap, in bytes. Ignored for a negated rule.
    pub max: u64,
}

/// What a capture does with one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict<'a> {
    /// It enters the Snapshot.
    Captured,
    /// Its name cannot be stored or must never be entered, whatever any rule says — the watched
    /// directory's own `.git` and its aliases.
    Reserved,
    /// A rule excludes it.
    Excluded(&'a Rule),
    /// An excluded ancestor keeps it from ever being reached (ADR 0007).
    Pruned {
        /// The excluded ancestor.
        ancestor: RootPath,
        /// The rule excluding the ancestor.
        rule: &'a Rule,
    },
    /// The built-in denylist refuses it as a likely secret, and no opt-in admits it.
    Secret(&'a Rule),
    /// A directory this Root does not own — a History Store, or another Root — fenced off by the daemon,
    /// so it is Withheld with that reason and never entered.
    Fenced(WithheldReason),
    /// It is over its cap, so it is Withheld `too_large`: present, but without its content.
    TooLarge {
        /// Its size.
        size: u64,
        /// The cap it is over.
        max: u64,
        /// The per-pattern cap that applied, or `None` for the scalar.
        rule: Option<&'a Rule>,
    },
}

/// The answer to "is this path captured, and why".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation<'a> {
    /// What a capture does with it.
    pub verdict: Verdict<'a>,
    /// The last of the user's rules matching the path itself — reported even when an ancestor prunes it
    /// or the denylist refuses it, so the answer says what that rule would have done.
    pub own_rule: Option<&'a Rule>,
    /// The denylist entry matching the path, whether or not an opt-in admitted it.
    pub secret: Option<&'a Rule>,
    /// The opt-in that admitted a path the denylist matches.
    pub opted_in: Option<&'a Rule>,
    /// The cap that applies to it.
    pub max_size: u64,
    /// The per-pattern cap that set it, if one did.
    pub size_rule: Option<&'a Rule>,
    /// Where the scalar cap was set: the built-in default or a configuration file's `max_size`.
    pub max_size_origin: &'a RuleOrigin,
}

/// Everything that decides what one Root captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePolicy {
    /// The user's Include Rules, and — on request — the directory's ignore files beneath them.
    rules: RuleSet,
    /// The paths the user opted in past the denylist.
    secret_opt_in: RuleSet,
    /// The cap for a file no per-pattern cap matches.
    max_size: u64,
    /// Where that cap was set.
    max_size_origin: RuleOrigin,
    /// The per-pattern caps, in precedence order.
    size_rules: Vec<SizeRule>,
    /// Whether the watched directory's own `.gitignore` files are honoured, beneath the user's rules.
    /// Off by default: `.gitignore` is an option rather than the law (ticket 04).
    honour_ignore_files: bool,
    secrets: RuleSet,
    /// Directories inside the Root that are not the Root's to capture, and why.
    fences: Vec<(RootPath, WithheldReason)>,
    /// The links followed rather than stored as links (ticket 49), by their own path.
    follow: RuleSet,
    /// The same directories as `fences`, and any others, by their real path: what a followed link may
    /// lead into, which no Root-relative path names.
    real_fences: Vec<(PathBuf, WithheldReason)>,
}

/// Why a link the follow list names is stored as a link after all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowStop {
    /// Its target is the Root, a directory the walk is already in, or one it reached through another
    /// followed link: following it would never end.
    Cycle,
    /// Its target is inside another Root or a History Store, which are not this Root's to capture.
    Inside(WithheldReason),
}

impl CapturePolicy {
    /// A policy from its parts; the denylist is built in and cannot be supplied.
    pub fn new(
        rules: RuleSet,
        secret_opt_in: RuleSet,
        max_size: u64,
        mut size_rules: Vec<SizeRule>,
    ) -> Self {
        size_rules.sort_by_key(|size| size.rule.origin.level);

        Self {
            rules,
            secret_opt_in,
            max_size,
            max_size_origin: RuleOrigin {
                level: Level::Default,
                source: "the built-in default".to_owned(),
                line: None,
            },
            size_rules,
            honour_ignore_files: false,
            secrets: secret_denylist(),
            fences: Vec::new(),
            follow: RuleSet::default(),
            real_fences: Vec::new(),
        }
    }

    /// This policy, saying where its scalar cap was set.
    pub fn with_max_size_origin(mut self, origin: RuleOrigin) -> Self {
        self.max_size_origin = origin;
        self
    }

    /// This policy, honouring the watched directory's own ignore files or not.
    pub fn honouring_ignore_files(mut self, honour: bool) -> Self {
        self.honour_ignore_files = honour;
        self
    }

    /// This policy, withholding each of `fences` — a directory inside the Root that something else owns —
    /// with its reason, whatever any rule says.
    ///
    /// The daemon knows where its History Stores and the other Roots are; a walk only knows names, so it
    /// is told rather than left to find out by capturing history into history.
    pub fn with_fences(mut self, fences: Vec<(RootPath, WithheldReason)>) -> Self {
        self.fences = fences;
        self
    }

    /// This policy, following the links `rules` match (`files.follow_symlinks`) rather than storing them.
    pub fn following(mut self, rules: RuleSet) -> Self {
        self.follow = rules;
        self
    }

    /// This policy, also withholding each of `fences` — a directory by its real path — where a followed
    /// link reaches it, and never following a link into one.
    pub fn with_real_fences(mut self, fences: Vec<(PathBuf, WithheldReason)>) -> Self {
        self.real_fences.extend(fences);
        self
    }

    /// Whether the link at `link`, whose target is of `kind`, is followed: the last follow pattern that
    /// matches it is not a negation. Include Rules still apply to the link's own path first.
    pub fn follows(&self, link: &RootPath, kind: Kind) -> bool {
        self.follow
            .last_match(link, kind)
            .is_some_and(|rule| !rule.negated)
    }

    /// Whether any link may be followed at all — which is what owes a Root the periodic Reconciliation
    /// of its followed targets.
    pub fn follows_any(&self) -> bool {
        self.follow.rules().any(|rule| !rule.negated)
    }

    /// Why a directory reached through a followed link, by its real path, is Withheld, if it is.
    pub fn fenced_at(&self, real: &Path) -> Option<WithheldReason> {
        self.real_fences
            .iter()
            .find(|(fenced, _)| fenced == real)
            .map(|(_, reason)| *reason)
    }

    /// Why following a link to the real path `target` stops, if it does, given `walk`: the Root's real
    /// path, every target already followed on the way here, and the real directory the link is in.
    ///
    /// A target that is any of them, or holds any of them, closes a cycle; one inside a fence leads
    /// where this Root has no business. Both are stored as links instead. A target that only *contains*
    /// a fence is followed, and the fence is withheld once reached ([`CapturePolicy::fenced_at`]).
    pub fn stop_following(&self, target: &Path, walk: &[PathBuf]) -> Option<FollowStop> {
        if let Some((_, reason)) = self
            .real_fences
            .iter()
            .find(|(fenced, _)| target.starts_with(fenced))
        {
            return Some(FollowStop::Inside(*reason));
        }

        walk.iter()
            .any(|walked| walked.starts_with(target))
            .then_some(FollowStop::Cycle)
    }

    /// Whether the watched directory's own ignore files join the rules.
    pub fn honours_ignore_files(&self) -> bool {
        self.honour_ignore_files
    }

    /// The Include Rules, for reporting on them.
    pub fn rules(&self) -> &RuleSet {
        &self.rules
    }

    /// This policy with the rules of an ignore file added beneath the user's own.
    pub fn with_ignore_rules(&self, rules: Vec<Rule>) -> Self {
        Self {
            rules: self.rules.extended(rules),
            ..self.clone()
        }
    }

    /// What a walk does with a directory it meets, its ancestors already entered.
    pub fn directory(&self, path: &RootPath) -> Verdict<'_> {
        self.own_verdict(path, Kind::Directory, None)
    }

    /// What a walk does with a file it meets, its ancestors already entered.
    pub fn file(&self, path: &RootPath, size: u64) -> Verdict<'_> {
        self.own_verdict(path, Kind::File, Some(size))
    }

    /// What a capture does with `path`, ancestors included, and why — the answer behind `explain`.
    ///
    /// `size` is `None` when the path is not on disk to be measured, and an unmeasured path is never
    /// called too large.
    pub fn explain(&self, path: &RootPath, kind: Kind, size: Option<u64>) -> Explanation<'_> {
        let classified = self.rules.classify(path, kind);

        let verdict = if path.ancestors().any(|ancestor| is_reserved(&ancestor)) {
            Verdict::Reserved
        } else if let Some(reason) = path.ancestors().find_map(|ancestor| self.fence(&ancestor)) {
            Verdict::Fenced(reason)
        } else if let Some((ancestor, rule)) = classified.pruned_by {
            Verdict::Pruned { ancestor, rule }
        } else {
            self.own_verdict(path, kind, size)
        };

        let (max_size, size_rule) = self.cap_for(path);
        let secret = match kind {
            Kind::File => self.secrets.last_match(path, kind),
            Kind::Directory => None,
        };

        Explanation {
            verdict,
            own_rule: classified.rule,
            secret,
            opted_in: secret.and_then(|_| self.opted_in(path)),
            max_size,
            size_rule,
            max_size_origin: &self.max_size_origin,
        }
    }

    /// The verdict on `path` itself, its ancestors assumed entered.
    fn own_verdict(&self, path: &RootPath, kind: Kind, size: Option<u64>) -> Verdict<'_> {
        if is_reserved(path) {
            return Verdict::Reserved;
        }
        if let Some(reason) = self.fence(path) {
            return Verdict::Fenced(reason);
        }
        if let Some(rule) = self.rules.last_match(path, kind) {
            if !rule.negated {
                return Verdict::Excluded(rule);
            }
        }
        if kind == Kind::Directory {
            return Verdict::Captured;
        }

        if let Some(secret) = self.secrets.last_match(path, kind) {
            if self.opted_in(path).is_none() {
                return Verdict::Secret(secret);
            }
        }

        let (max, rule) = self.cap_for(path);
        match size {
            Some(size) if size > max => Verdict::TooLarge { size, max, rule },
            _ => Verdict::Captured,
        }
    }

    /// Why `path` is fenced off, if it is.
    fn fence(&self, path: &RootPath) -> Option<WithheldReason> {
        self.fences
            .iter()
            .find(|(fenced, _)| fenced == path)
            .map(|(_, reason)| *reason)
    }

    /// This policy with what Forget excluded from now on (ticket 81), at its own level above every user
    /// level, so that it outranks whatever the user's files say about the same paths.
    pub fn with_forgotten(&self, rules: Vec<Rule>) -> Self {
        Self {
            rules: self.rules.extended(rules),
            ..self.clone()
        }
    }

    /// The opt-in admitting `path` past the denylist, if one does — which is also what makes a path one
    /// Forget may not exclude: the user has said this secret is to be captured (ticket 06).
    pub fn opted_in(&self, path: &RootPath) -> Option<&Rule> {
        self.secret_opt_in
            .last_match(path, Kind::File)
            .filter(|rule| !rule.negated)
    }

    /// The cap for `path`, and the per-pattern rule that set it, if one did.
    ///
    /// A cap written for a directory covers the files beneath it — `videos/` is how a person says "the
    /// videos" — so a rule applies when it matches the file or any directory above it.
    fn cap_for(&self, path: &RootPath) -> (u64, Option<&Rule>) {
        match self.size_rules.iter().rev().find(|size| {
            size.rule.matches(path, Kind::File)
                || path
                    .ancestors()
                    .any(|ancestor| size.rule.matches(&ancestor, Kind::Directory))
        }) {
            Some(size) if !size.rule.negated => (size.max, Some(&size.rule)),
            _ => (self.max_size, None),
        }
    }
}

impl Default for CapturePolicy {
    /// What a user who configures nothing gets: no rules of their own, secrets refused, and the
    /// 100 MiB cap.
    fn default() -> Self {
        Self::new(
            RuleSet::default(),
            RuleSet::default(),
            DEFAULT_MAX_SIZE,
            Vec::new(),
        )
    }
}

/// The built-in denylist, as rules at the default level so `explain` can name the one that refused.
fn secret_denylist() -> RuleSet {
    RuleSet::new(
        SECRET_NAMES
            .iter()
            .map(|name| {
                Rule::parse(
                    name,
                    RootPath::root(),
                    RuleOrigin {
                        level: Level::Default,
                        source: SECRET_SOURCE.to_owned(),
                        line: None,
                    },
                )
                .expect("the built-in denylist is valid")
            })
            .collect(),
    )
}

/// Whether the last component of `path` is a name that can never be stored.
///
/// `.git` and its platform aliases: libgit2's owner-less index refuses such a path on every platform,
/// and the map rules out any interaction with a watched directory's own git, so it is not entered
/// either — every git command would otherwise churn its index and objects into history (ticket 10).
/// And the sidecar's own name at the top of the Root, where the Snapshot keeps its records (ticket 48).
pub fn is_reserved(path: &RootPath) -> bool {
    if path.as_bytes() == SIDECAR.as_bytes() {
        return true;
    }
    // Both names are ASCII, so folding ASCII alone decides it whatever other bytes a name holds.
    let folded = path.name().to_ascii_lowercase();
    let end = folded
        .iter()
        .rposition(|byte| !matches!(byte, b'.' | b' '))
        .map_or(0, |last| last + 1);
    let trimmed = &folded[..end];

    trimmed == b".git" || trimmed == b"git~1"
}

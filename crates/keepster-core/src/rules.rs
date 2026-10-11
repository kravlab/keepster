//! Include Rules: layered pattern lists in git's ignore syntax, evaluated as values.
//!
//! A rule *excludes* what it matches, and a negated rule (`!pattern`) re-includes it; among the rules
//! that match a path, the last one wins, with the levels ordered from the least specific to the most.
//! The one exception is ADR 0007's: **an excluded directory is never entered**, so no rule beneath it is
//! ever evaluated and a negation there can bring nothing back. That is git's rule, adopted because the
//! watches are installed after the rules are applied; git's *silence* is not adopted with it —
//! [`RuleSet::classify`] names the pruning ancestor and [`RuleSet::unreachable_negations`] reports every
//! negation an ancestor makes unreachable.

use std::fmt;

use crate::path::RootPath;

/// Where a rule was written, from the least specific level to the most.
///
/// The order of the variants is the precedence: a later level outranks an earlier one whatever order
/// the rules were handed over in. `IgnoreFile` — a watched directory's own `.gitignore`, honoured only on
/// request — sits beneath every level the user writes, because their rules are authoritative and
/// `.gitignore` is an option rather than the law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Built into Keepster.
    Default,
    /// A watched directory's own ignore file.
    IgnoreFile,
    /// The global configuration.
    Global,
    /// A pattern block of the global configuration, addressed by a path glob (ticket 42).
    GlobalPattern,
    /// A Root's own block of the global configuration (ticket 42).
    GlobalRoot,
    /// The Root's own `.keepster.toml`.
    Root,
}

impl Level {
    /// The level's name, as provenance spells it on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::IgnoreFile => "ignore-file",
            Self::Global => "global",
            Self::GlobalPattern => "global-pattern",
            Self::GlobalRoot => "global-root",
            Self::Root => "root",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a path is a file or a directory, which a pattern ending in `/` cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Anything that is not a directory.
    File,
    /// A directory.
    Directory,
}

/// Where one rule came from, so `explain` can name the rule a person wrote rather than describe a
/// computation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleOrigin {
    /// The level it was written at.
    pub level: Level,
    /// The file it was written in, or a description of a built-in list.
    pub source: String,
    /// Its line in that file, where it has one.
    pub line: Option<u32>,
}

/// Why a pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{pattern}` is not a valid pattern: {reason}")]
pub struct InvalidPattern {
    /// The pattern as written.
    pub pattern: String,
    /// What is wrong with it.
    pub reason: &'static str,
}

/// One pattern, compiled, with where it came from.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The pattern exactly as written, including any `!`.
    pub text: String,
    /// Whether it re-includes what it matches rather than excluding it.
    pub negated: bool,
    /// Where it was written.
    pub origin: RuleOrigin,
    /// The directory its pattern is relative to: the Root for configuration, the ignore file's own
    /// directory for an ignore file.
    base: RootPath,
    /// Whether it matches directories only (a trailing `/`).
    directories_only: bool,
    /// Whether it is anchored to its base, rather than matching a name at any depth.
    anchored: bool,
    /// The pattern without `!`, a leading `/` and a trailing `/`.
    body: String,
    matcher: regex::Regex,
}

impl PartialEq for Rule {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.origin == other.origin && self.base == other.base
    }
}

impl Eq for Rule {}

impl Rule {
    /// Compile one pattern in git's ignore syntax, relative to `base`.
    ///
    /// `*` and `?` stop at `/`, `[…]` is a class (`[!…]` its complement), `**` crosses directories, a
    /// `/` anywhere but the end anchors the pattern to its base, a trailing `/` matches directories
    /// only, `!` negates, and `\` escapes the next character.
    pub fn parse(text: &str, base: RootPath, origin: RuleOrigin) -> Result<Self, InvalidPattern> {
        let invalid = |reason| InvalidPattern {
            pattern: text.to_owned(),
            reason,
        };

        let (negated, rest) = match text.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (directories_only, rest) = match rest.strip_suffix('/') {
            Some(rest) if !rest.ends_with('\\') => (true, rest),
            _ => (false, rest),
        };
        let (leading_slash, body) = match rest.strip_prefix('/') {
            Some(body) => (true, body),
            None => (false, rest),
        };
        if body.is_empty() {
            return Err(invalid("it matches nothing"));
        }

        let anchored = leading_slash || body.contains('/');
        let translated = translate(body).map_err(invalid)?;
        let expression = if anchored {
            format!("^{translated}$")
        } else {
            format!("^(?:.*/)?{translated}$")
        };
        let matcher =
            regex::Regex::new(&expression).map_err(|_| invalid("it cannot be compiled"))?;

        Ok(Self {
            text: text.to_owned(),
            negated,
            origin,
            base,
            directories_only,
            anchored,
            body: body.to_owned(),
            matcher,
        })
    }

    /// Compile an entry of an `include` list: a pattern that re-includes what it matches.
    ///
    /// Within one level an `include` entry is the negation `!pattern` evaluated after that level's
    /// `exclude` list, so a level's own exceptions win over its own exclude rules; the pattern keeps the
    /// spelling it was written with, and a leading `!` — which would mean excluding — is refused.
    pub fn include(text: &str, base: RootPath, origin: RuleOrigin) -> Result<Self, InvalidPattern> {
        if text.starts_with('!') {
            return Err(InvalidPattern {
                pattern: text.to_owned(),
                reason: "an `include` entry cannot be negated; write it in `exclude` instead",
            });
        }

        let mut rule = Self::parse(&format!("!{text}"), base, origin)?;
        rule.text = text.to_owned();
        Ok(rule)
    }

    /// Whether the pattern matches `path`, ignoring what the match means.
    pub fn matches(&self, path: &RootPath, kind: Kind) -> bool {
        if self.directories_only && kind != Kind::Directory {
            return false;
        }

        match relative_to(path, &self.base) {
            Some(relative) if !relative.is_empty() => self.matcher.is_match(relative),
            _ => false,
        }
    }

    /// The directories an anchored pattern names literally before its first wildcard, as paths from
    /// the Root — the ancestors that must be entered for the pattern ever to be reached.
    fn literal_ancestors(&self) -> Vec<RootPath> {
        if !self.anchored {
            return Vec::new();
        }

        let components: Vec<&str> = self.body.split('/').collect();
        let mut ancestors = Vec::new();
        let mut current = self.base.clone();
        // The last component is the target itself, not one of its ancestors.
        for component in &components[..components.len() - 1] {
            if component.contains(['*', '?', '[', '\\']) {
                break;
            }
            current = current.child(component);
            ancestors.push(current.clone());
        }

        ancestors
    }
}

/// The name of the ignore file a watched directory may carry, honoured only on request.
pub const IGNORE_FILE: &str = ".gitignore";

/// The rules of one ignore file in a watched directory, relative to the directory holding it.
///
/// git's line syntax: blank lines and lines starting with `#` say nothing, trailing whitespace is
/// dropped, and a line that is not a valid pattern is passed over as git passes it over — the file
/// belongs to the directory's own tooling, not to Keepster, so its mistakes are not ours to refuse.
pub fn ignore_file_rules(directory: &RootPath, text: &str, source: &str) -> Vec<Rule> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = trim_unescaped_trailing_space(line);
            if line.is_empty() || line.starts_with('#') {
                return None;
            }

            Rule::parse(
                &line,
                directory.clone(),
                RuleOrigin {
                    level: Level::IgnoreFile,
                    source: source.to_owned(),
                    line: u32::try_from(index + 1).ok(),
                },
            )
            .ok()
        })
        .collect()
}

/// `line` without its trailing whitespace, except a space escaped with `\\`, which git keeps.
fn trim_unescaped_trailing_space(line: &str) -> String {
    let trimmed = line.trim_end();
    if trimmed.ends_with('\\') && trimmed.len() < line.len() {
        format!("{trimmed} ")
    } else {
        trimmed.to_owned()
    }
}

/// A glob over whole directory paths, which is how a pattern block of the global configuration names
/// the Roots it applies to (ticket 04).
///
/// The syntax is the rules' own — `*`, `?` and `[…]` stop at `/`, `**/` and a trailing `/**` cross
/// directories, `\` escapes — so a person learns one glob language. The pattern is matched against the
/// whole path, `/`-separated, from its first character: it describes a directory, not a name at any
/// depth.
#[derive(Debug, Clone)]
pub struct PathGlob {
    text: String,
    matcher: regex::Regex,
}

impl PathGlob {
    /// Compile `text`, which must name an absolute path once a home directory has been expanded.
    pub fn parse(text: &str) -> Result<Self, InvalidPattern> {
        let invalid = |reason| InvalidPattern {
            pattern: text.to_owned(),
            reason,
        };

        let body = text.strip_suffix('/').unwrap_or(text);
        if body.is_empty() {
            return Err(invalid("it matches nothing"));
        }
        let expression = format!("^{}$", translate(body).map_err(invalid)?);
        let matcher =
            regex::Regex::new(&expression).map_err(|_| invalid("it cannot be compiled"))?;

        Ok(Self {
            text: text.to_owned(),
            matcher,
        })
    }

    /// Whether the glob matches `path`, written with `/` separators.
    pub fn matches(&self, path: &str) -> bool {
        self.matcher.is_match(path)
    }

    /// The glob as written.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

/// `path` relative to `base`, or `None` when it is not beneath it.
fn relative_to<'a>(path: &'a RootPath, base: &RootPath) -> Option<&'a str> {
    if base.is_root() {
        return Some(path.as_str());
    }

    path.as_str()
        .strip_prefix(base.as_str())
        .and_then(|rest| rest.strip_prefix('/'))
}

/// Translate a glob body into a regular expression body.
fn translate(body: &str) -> Result<String, &'static str> {
    let characters: Vec<char> = body.chars().collect();
    let mut expression = String::new();
    let mut index = 0;

    while index < characters.len() {
        let character = characters[index];
        match character {
            '*' if characters.get(index + 1) == Some(&'*') => {
                let at_start = index == 0 || characters[index - 1] == '/';
                let next = characters.get(index + 2);
                if at_start && next == Some(&'/') {
                    // `**/` — zero or more whole directories.
                    expression.push_str("(?:.*/)?");
                    index += 3;
                } else if at_start && next.is_none() {
                    // A trailing `/**` — everything beneath, but not the directory itself.
                    expression.push_str(".+");
                    index += 2;
                } else {
                    // Anywhere else, git reads consecutive stars as one ordinary `*`.
                    expression.push_str("[^/]*");
                    while characters.get(index) == Some(&'*') {
                        index += 1;
                    }
                }
            }
            '*' => {
                expression.push_str("[^/]*");
                index += 1;
            }
            '?' => {
                expression.push_str("[^/]");
                index += 1;
            }
            '[' => {
                let (class, next) = bracket(&characters, index)?;
                // `/` never matches a class, as it never matches a wildcard: the class is intersected
                // with everything but `/`.
                expression.push_str("[[");
                expression.push_str(&class);
                expression.push_str("]&&[^/]]");
                index = next;
            }
            '\\' => {
                let escaped = characters.get(index + 1).ok_or("it ends in a lone `\\`")?;
                expression.push_str(&regex::escape(&escaped.to_string()));
                index += 2;
            }
            other => {
                expression.push_str(&regex::escape(&other.to_string()));
                index += 1;
            }
        }
    }

    Ok(expression)
}

/// Translate the bracket expression starting at `characters[open]`, returning the regular expression
/// class body and the index just past its closing `]`.
///
/// git's rules: `!` (or `^`) first negates, a `]` first is literal, `\` escapes the next character, and a
/// named class such as `[:digit:]` is copied whole, because its own `]` does not close the expression.
fn bracket(characters: &[char], open: usize) -> Result<(String, usize), &'static str> {
    let mut class = String::new();
    let mut index = open + 1;

    if matches!(characters.get(index), Some('!' | '^')) {
        class.push('^');
        index += 1;
    }
    let first = index;

    loop {
        let character = *characters.get(index).ok_or("a `[` is never closed")?;
        match character {
            ']' if index > first => return Ok((class, index + 1)),
            '[' if characters.get(index + 1) == Some(&':') => {
                let rest: String = characters[index..].iter().collect();
                let end = rest.find(":]").ok_or("a `[:` class is never closed")?;
                let named = &rest[..end + 2];
                class.push_str(named);
                index += named.chars().count();
            }
            '\\' => {
                let escaped = characters.get(index + 1).ok_or("it ends in a lone `\\`")?;
                class.push_str(&regex::escape(&escaped.to_string()));
                index += 2;
            }
            // Characters with a meaning of their own inside a regular-expression class.
            '[' | ']' | '&' | '~' => {
                class.push('\\');
                class.push(character);
                index += 1;
            }
            other => {
                class.push(other);
                index += 1;
            }
        }
    }
}

/// The rules that decide what a Root captures, in precedence order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

/// What the rules say about one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification<'a> {
    /// The excluded ancestor that keeps the path from ever being reached, and the rule excluding it.
    pub pruned_by: Option<(RootPath, &'a Rule)>,
    /// The last rule matching the path itself, if any — reported even when an ancestor prunes it, so
    /// `explain` can say what would have re-included it.
    pub rule: Option<&'a Rule>,
}

impl Classification<'_> {
    /// Whether the rules let the path into a Snapshot.
    pub fn captured(&self) -> bool {
        self.pruned_by.is_none() && self.rule.is_none_or(|rule| rule.negated)
    }
}

/// A negation that can never take effect, because an ancestor it needs is excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreachableNegation<'a> {
    /// The negation.
    pub negation: &'a Rule,
    /// The excluded ancestor.
    pub ancestor: RootPath,
    /// The rule excluding it.
    pub excluded_by: &'a Rule,
}

impl RuleSet {
    /// A rule set from rules in any order of level; within one level, their order is kept.
    pub fn new(mut rules: Vec<Rule>) -> Self {
        rules.sort_by_key(|rule| rule.origin.level);
        Self { rules }
    }

    /// This rule set with more rules added, each placed by its level after the rules already there.
    ///
    /// This is how a walk adds a directory's ignore file as it enters the directory: a deeper ignore
    /// file outranks a shallower one, as in git.
    pub fn extended(&self, more: Vec<Rule>) -> Self {
        let mut rules = self.rules.clone();
        rules.extend(more);
        Self::new(rules)
    }

    /// The last rule matching `path` itself, ignoring its ancestors.
    ///
    /// This is what a walk asks about each entry it meets: it never meets an entry beneath an excluded
    /// directory, so the ancestors have already been decided.
    pub fn last_match(&self, path: &RootPath, kind: Kind) -> Option<&Rule> {
        self.rules
            .iter()
            .rev()
            .find(|rule| rule.matches(path, kind))
    }

    /// What the rules say about `path`, ancestors included.
    pub fn classify(&self, path: &RootPath, kind: Kind) -> Classification<'_> {
        let pruned_by = path.ancestors().find_map(|ancestor| {
            self.last_match(&ancestor, Kind::Directory)
                .filter(|rule| !rule.negated)
                .map(|rule| (ancestor, rule))
        });

        Classification {
            pruned_by,
            rule: self.last_match(path, kind),
        }
    }

    /// Every negation that an excluded ancestor makes unreachable.
    ///
    /// Only what can be known without a walk is reported: the directories an anchored negation names
    /// literally. A negation such as `!*.keep` has no such ancestors, and `explain` is the answer for it.
    pub fn unreachable_negations(&self) -> Vec<UnreachableNegation<'_>> {
        self.rules
            .iter()
            .filter(|rule| rule.negated)
            .filter_map(|negation| {
                negation
                    .literal_ancestors()
                    .into_iter()
                    .find_map(|ancestor| {
                        self.last_match(&ancestor, Kind::Directory)
                            .filter(|rule| !rule.negated)
                            .map(|excluded_by| UnreachableNegation {
                                negation,
                                ancestor,
                                excluded_by,
                            })
                    })
            })
            .collect()
    }
}

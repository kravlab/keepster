//! What a search matches (ticket 59, ticket 34): a path by a fragment of its name or by an Include Rules
//! pattern, and the content of a version by a literal or a regular expression — matched case-insensitively
//! unless the query has a capital in it, and always in linear time, since anyone who can reach the API can
//! send a query and none may hang the daemon.

use crate::path::RootPath;
use crate::rules::{Kind, Level, Rule, RuleOrigin};

/// How many bytes are looked at for a NUL before a version is taken for binary, as git and ripgrep do.
const BINARY_PROBE: usize = 8000;

/// The longest a snippet is, in characters, around what matched.
const SNIPPET: usize = 160;

/// The most a compiled expression may take, so that one query cannot claim the daemon's memory.
const COMPILED_LIMIT: usize = 1 << 20;

/// Why a query cannot be searched for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct InvalidQuery(String);

/// Whether a query has a capital in it, which makes it match case-sensitively.
fn smart_case(text: &str) -> bool {
    text.chars().any(char::is_uppercase)
}

/// Whether a regular expression has a capital among what it matches — not counting what follows a
/// backslash, so that `\S`, `\W` or `\P{Greek}` asks for no case, as ripgrep reads them.
fn smart_case_expression(expression: &str) -> bool {
    let mut characters = expression.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            if character.is_uppercase() {
                return true;
            }
            continue;
        }
        // The escaped character names no case, nor the class after `\p` or `\P`: one letter, or a
        // name in braces.
        if let Some('p' | 'P') = characters.next() {
            if characters.next() == Some('{') {
                characters
                    .by_ref()
                    .take_while(|inner| *inner != '}')
                    .for_each(drop);
            }
        }
    }

    false
}

/// A path query: a fragment of a path, or an Include Rules pattern.
#[derive(Debug, Clone)]
pub enum PathQuery {
    /// A fragment, anywhere in the path; `folded` when it is matched without regard to case.
    Fragment {
        /// The fragment, folded when `folded`.
        text: String,
        /// Whether case is ignored.
        folded: bool,
    },
    /// A pattern, read as an Include Rule written at the Root (ADR 0007): one pattern language. It finds
    /// what it matches and everything beneath a directory it matches, as an Include Rule takes them in.
    Pattern {
        /// The rule.
        rule: Rule,
        /// Whether case is ignored.
        folded: bool,
    },
}

impl PathQuery {
    /// A query for `text`: a fragment, or with `pattern` an Include Rules pattern.
    pub fn new(text: &str, pattern: bool) -> Result<Self, InvalidQuery> {
        if text.is_empty() {
            return Err(InvalidQuery("the query is empty".to_owned()));
        }
        if !pattern {
            let folded = !smart_case(text);
            return Ok(Self::Fragment {
                text: if folded {
                    text.to_lowercase()
                } else {
                    text.to_owned()
                },
                folded,
            });
        }
        if text.starts_with('!') {
            return Err(InvalidQuery(
                "a pattern to find cannot be negated: it would name what to leave out".to_owned(),
            ));
        }
        let origin = RuleOrigin {
            level: Level::Global,
            source: "search".to_owned(),
            line: None,
        };

        // A pattern with no capital is already its own folded form; only the paths need folding.
        let folded = !smart_case(text);

        Rule::parse(text, RootPath::root(), origin)
            .map(|rule| Self::Pattern { rule, folded })
            .map_err(|error| InvalidQuery(error.to_string()))
    }

    /// Whether `path` is found.
    ///
    /// A query is text, so it is matched against the path as text, a name that is not UTF-8 read with
    /// U+FFFD — as [`Rule::matches`] reads it.
    pub fn matches(&self, path: &RootPath) -> bool {
        let text = path.to_lossy();
        match self {
            Self::Fragment {
                text: query,
                folded: true,
            } => text.to_lowercase().contains(query.as_str()),
            Self::Fragment { text: query, .. } => text.contains(query.as_str()),
            Self::Pattern { rule, folded } => {
                let folded_path;
                let path = if *folded {
                    folded_path = RootPath::parse(&text.to_lowercase());
                    match &folded_path {
                        Ok(folded) => folded,
                        Err(_) => return false,
                    }
                } else {
                    path
                };
                rule.matches(path, Kind::File)
                    || path
                        .ancestors()
                        .any(|directory| rule.matches(&directory, Kind::Directory))
            }
        }
    }
}

/// A content query: a literal, or a regular expression, compiled for byte content.
#[derive(Debug, Clone)]
pub struct ContentQuery {
    matcher: regex::bytes::Regex,
}

/// One line of a version that matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// The line, counted from 1.
    pub line: u64,
    /// The text around what matched, at most [`SNIPPET`] characters, lossily decoded.
    pub snippet: String,
}

impl ContentQuery {
    /// A query for `text`: a literal, or with `regex` a regular expression.
    pub fn new(text: &str, regex: bool) -> Result<Self, InvalidQuery> {
        if text.is_empty() {
            return Err(InvalidQuery("the query is empty".to_owned()));
        }
        let expression = if regex {
            text.to_owned()
        } else {
            regex::escape(text)
        };
        let matcher = regex::bytes::RegexBuilder::new(&expression)
            .case_insensitive(if regex {
                !smart_case_expression(text)
            } else {
                !smart_case(text)
            })
            .size_limit(COMPILED_LIMIT)
            .dfa_size_limit(COMPILED_LIMIT)
            .build()
            .map_err(|error| InvalidQuery(error.to_string()))?;

        Ok(Self { matcher })
    }

    /// Every line of `content` that matches, in order — each without its ending, `\n` or `\r\n`, so that
    /// `$` means the end of the line whichever ending it has, and with no empty line after the last one.
    pub fn lines(&self, content: &[u8]) -> Vec<LineMatch> {
        let content = content.strip_suffix(b"\n").unwrap_or(content);
        content
            .split(|byte| *byte == b'\n')
            .zip(1_u64..)
            .filter_map(|(line, number)| {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                let found = self.matcher.find(line)?;
                Some(LineMatch {
                    line: number,
                    snippet: snippet(line, found.start(), found.end()),
                })
            })
            .collect()
    }
}

/// Whether `content` is binary: a NUL among its first bytes. Such a version is skipped and counted, never
/// searched as text.
pub fn is_binary(content: &[u8]) -> bool {
    content[..content.len().min(BINARY_PROBE)].contains(&0)
}

/// The text of `line` around the match from `start` to `end`, trimmed of its surrounding space, and
/// shortened to [`SNIPPET`] characters with the match kept in view.
fn snippet(line: &[u8], start: usize, end: usize) -> String {
    let text = String::from_utf8_lossy(line);
    let characters: Vec<char> = text.chars().collect();
    if characters.len() <= SNIPPET {
        return text.trim().to_owned();
    }
    // Where the match starts, in characters of the lossy text.
    let at = String::from_utf8_lossy(&line[..start.min(line.len())])
        .chars()
        .count();
    let length = String::from_utf8_lossy(&line[start.min(line.len())..end.min(line.len())])
        .chars()
        .count();
    let before = SNIPPET.saturating_sub(length) / 2;
    let from = at.saturating_sub(before);
    let to = (from + SNIPPET).min(characters.len());
    let from = to.saturating_sub(SNIPPET);

    let mut shown: String = characters[from..to].iter().collect();
    if from > 0 {
        shown.insert(0, '…');
    }
    if to < characters.len() {
        shown.push('…');
    }
    shown.trim().to_owned()
}

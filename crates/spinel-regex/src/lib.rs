//! Ruby's regex dialect, from scratch in Rust.
//!
//! # Why this crate exists
//!
//! Ruby's regexes are Onigmo's, and Onigmo is neither PCRE nor Rust's `regex`.
//! Before writing any of this, `scripts/regexp-oracle.rb` took every pattern
//! `ruby/spec`'s `language/regexp/` actually uses, ran each one against a probe
//! corpus on CRuby, and replayed them through `fancy-regex` with the fairest
//! translation available. Of 338 patterns CRuby accepts, 281 agreed, 16 were
//! rejected, and **41 produced a different answer**. Some of those 41 are
//! reachable by translation — `\w` and `\s` are ASCII in Ruby and Unicode in
//! Rust, the POSIX brackets are the other way round — but the rest are
//! properties of the match engine itself, and no wrapper reaches them:
//!
//! ```text
//! /(a*)*/  on "a"     ruby: group 1 is ""     fancy-regex: group 1 is "a"
//! /(a|\2b|())*/ "ab"  ruby: matches 2 chars   fancy-regex: matches 1
//! ```
//!
//! A backend that silently answers differently for one pattern in eight is the
//! plausible-but-wrong answer `docs/engine.md` refuses. So: a parser for the
//! real dialect, and a backtracking machine behind it.
//!
//! # What it refuses
//!
//! Constructs this engine does not implement yet return [`Error::Unsupported`]
//! rather than a guess. The VM turns that into `Error::Unknowable`, and the
//! spec harness reports the example *blocked* — never passed, never failed.
//! Today that is `(?~)`, `\g<>`, conditional groups, `\K`, `\R`, `\X`, `\p{}`
//! and `\k<name+1>` level specifiers.
//!
//! ```
//! use spinel_regex::{Flags, Regex};
//!
//! let re = Regex::new("h(?<mid>.)llo", Flags::default()).unwrap();
//! let m = re.find_at("say hello", 0).unwrap().unwrap();
//! assert_eq!(m.group(0), Some((4, 9)));
//! assert_eq!(re.name_to_group("mid"), Some(1));
//! ```

mod exec;
mod parse;

pub use exec::Match as RawMatch;

use std::fmt;

/// What can go wrong, either when compiling a pattern or when running one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The pattern is not valid Ruby. Becomes `RegexpError`, or `SyntaxError`
    /// for a literal the compiler saw.
    Syntax(String),
    /// A construct this engine has not implemented. Never a wrong answer.
    Unsupported(&'static str),
    /// Backtracking ran past its step budget. The VM reports this the way it
    /// reports any other budget exhaustion, rather than hanging.
    Budget,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax(message) => write!(f, "{message}"),
            Error::Unsupported(what) => write!(f, "{what} is not supported yet"),
            Error::Budget => write!(f, "regexp match took too many steps"),
        }
    }
}

impl std::error::Error for Error {}

/// The three flags a Ruby regexp literal can carry.
///
/// Ruby's `/m` means "dot matches newline", which is Rust's `s` and not Rust's
/// `m`; `^` and `$` are line anchors in Ruby with or without it. The field is
/// named for what it does rather than for the letter that spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    /// `/i`
    pub icase: bool,
    /// `/x` — whitespace and `#` comments in the pattern are not pattern.
    pub extended: bool,
    /// `/m` — `.` also matches a newline.
    pub dotall: bool,
}

impl Flags {
    /// Ruby's `Regexp::IGNORECASE`.
    pub const IGNORECASE: i64 = 1;
    /// Ruby's `Regexp::EXTENDED`.
    pub const EXTENDED: i64 = 2;
    /// Ruby's `Regexp::MULTILINE`.
    pub const MULTILINE: i64 = 4;

    /// What `Regexp#options` answers.
    #[must_use]
    pub fn to_options(self) -> i64 {
        let mut options = 0;
        if self.icase {
            options |= Self::IGNORECASE;
        }
        if self.extended {
            options |= Self::EXTENDED;
        }
        if self.dotall {
            options |= Self::MULTILINE;
        }
        options
    }

    /// The inverse, for `Regexp.new(source, options)`.
    #[must_use]
    pub fn from_options(options: i64) -> Flags {
        Flags {
            icase: options & Self::IGNORECASE != 0,
            extended: options & Self::EXTENDED != 0,
            dotall: options & Self::MULTILINE != 0,
        }
    }

    /// The letters Ruby prints in `/foo/mix`, in Ruby's order.
    #[must_use]
    pub fn to_letters(self) -> String {
        let mut letters = String::new();
        if self.dotall {
            letters.push('m');
        }
        if self.icase {
            letters.push('i');
        }
        if self.extended {
            letters.push('x');
        }
        letters
    }
}

/// One compiled pattern.
pub struct Regex {
    source: String,
    flags: Flags,
    program: exec::Program,
    names: Vec<(String, usize)>,
}

impl fmt::Debug for Regex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}/{}", self.source, self.flags.to_letters())
    }
}

impl Regex {
    /// Compile `source` under `flags`.
    ///
    /// # Errors
    ///
    /// [`Error::Syntax`] if the pattern is not valid Ruby, or
    /// [`Error::Unsupported`] for a construct this engine refuses rather than
    /// approximates.
    pub fn new(source: &str, flags: Flags) -> Result<Regex, Error> {
        let parsed = parse::parse(source, flags)?;
        let program = exec::compile(&parsed.ast, parsed.groups)?;
        Ok(Regex {
            source: source.to_owned(),
            flags,
            program,
            names: parsed.names,
        })
    }

    /// The pattern as written, which is what `Regexp#source` answers.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn flags(&self) -> Flags {
        self.flags
    }

    /// How many capturing groups the pattern has, not counting the whole match.
    #[must_use]
    pub fn group_count(&self) -> usize {
        self.program.groups
    }

    /// Every `(?<name>)` in source order.
    #[must_use]
    pub fn names(&self) -> &[(String, usize)] {
        &self.names
    }

    /// The group a name refers to, when only one does.
    #[must_use]
    pub fn name_to_group(&self, name: &str) -> Option<usize> {
        self.groups_named(name).last().copied()
    }

    /// Every group a name refers to, in source order.
    ///
    /// Ruby lets one name label several groups — `/(?:A(?<w>\w+)|B(?<w>\w+))/`
    /// — and `MatchData#[:w]` answers whichever of them took part, so the
    /// caller needs the whole list rather than one index.
    #[must_use]
    pub fn groups_named(&self, name: &str) -> Vec<usize> {
        self.names
            .iter()
            .filter(|(n, _)| n == name)
            .map(|&(_, group)| group)
            .collect()
    }

    /// The leftmost match at or after byte offset `start`.
    ///
    /// # Errors
    ///
    /// [`Error::Budget`] if backtracking ran past its step budget.
    pub fn find_at(&self, haystack: &str, start: usize) -> Result<Option<Captures>, Error> {
        Ok(self
            .program
            .find_at(haystack, start)?
            .map(|m| Captures { caps: m.caps }))
    }

    /// Whether the pattern matches anywhere in `haystack`, which is what
    /// `Regexp#match?` answers without building a `MatchData`.
    ///
    /// # Errors
    ///
    /// [`Error::Budget`] if backtracking ran past its step budget.
    pub fn is_match(&self, haystack: &str) -> Result<bool, Error> {
        Ok(self.find_at(haystack, 0)?.is_some())
    }
}

/// Where every group landed, in byte offsets into the subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captures {
    caps: Vec<Option<(usize, usize)>>,
}

impl Captures {
    /// Group `n` as a byte range, or `None` if it did not take part.
    /// Group 0 is the whole match.
    #[must_use]
    pub fn group(&self, n: usize) -> Option<(usize, usize)> {
        self.caps.get(n).copied().flatten()
    }

    /// How many groups there are, counting group 0.
    #[must_use]
    pub fn len(&self) -> usize {
        self.caps.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.caps.is_empty()
    }
}

/// `Regexp.escape` — the string, with every metacharacter quoted.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            // The set CRuby's `rb_reg_quote` escapes, spelled out.
            '[' | ']' | '{' | '}' | '(' | ')' | '|' | '-' | '*' | '.' | '\\' | '?' | '+' | '^'
            | '$' | '#' | '/' => {
                out.push('\\');
                out.push(c);
            }
            ' ' => out.push_str("\\ "),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x0c' => out.push_str("\\f"),
            '\x0b' => out.push_str("\\v"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pattern: &str, haystack: &str) -> Option<(usize, usize)> {
        Regex::new(pattern, Flags::default())
            .expect("compiles")
            .find_at(haystack, 0)
            .expect("runs")
            .and_then(|c| c.group(0))
    }

    #[test]
    fn literals_and_alternation() {
        assert_eq!(find("foo", "a foo b"), Some((2, 5)));
        assert_eq!(find("foo|bar", "a bar"), Some((2, 5)));
        assert_eq!(find("zzz", "a foo b"), None);
    }

    #[test]
    fn caret_and_dollar_are_line_anchors_without_any_flag() {
        // Rust's regex needs `(?m)` for this; Ruby never does. The single
        // most load-bearing difference between the two dialects.
        assert_eq!(find("^bar", "foo\nbar"), Some((4, 7)));
        assert_eq!(find("foo$", "foo\nbar"), Some((0, 3)));
        assert_eq!(find("\\Abar", "foo\nbar"), None);
        assert_eq!(find("\\Afoo", "foo\nbar"), Some((0, 3)));
    }

    #[test]
    fn z_upper_allows_one_trailing_newline_and_z_lower_does_not() {
        assert_eq!(find("foo\\Z", "foo\n"), Some((0, 3)));
        assert_eq!(find("foo\\z", "foo\n"), None);
        assert_eq!(find("foo\\z", "foo"), Some((0, 3)));
    }

    #[test]
    fn dot_skips_newline_unless_dotall() {
        assert_eq!(find("a.b", "a\nb"), None);
        let re = Regex::new(
            "a.b",
            Flags {
                dotall: true,
                ..Flags::default()
            },
        )
        .expect("compiles");
        assert!(re.is_match("a\nb").expect("runs"));
    }

    #[test]
    fn perl_shorthands_are_ascii_and_posix_brackets_are_unicode() {
        // The inversion `scripts/regexp-oracle.rb` measured against CRuby.
        assert_eq!(find("\\w", "é"), None);
        assert_eq!(find("[[:alpha:]]", "é"), Some((0, 2)));
        assert_eq!(find("\\d", "٣"), None);
        assert_eq!(find("[[:digit:]]", "٣"), Some((0, 2)));
    }

    #[test]
    fn greedy_and_lazy_quantifiers_differ() {
        assert_eq!(find("<.+>", "<a><b>"), Some((0, 6)));
        assert_eq!(find("<.+?>", "<a><b>"), Some((0, 3)));
    }

    #[test]
    fn backreference_matches_what_the_group_took() {
        assert_eq!(find("(ab)\\1", "abab"), Some((0, 4)));
        assert_eq!(find("(ab)\\1", "abcd"), None);
        // "fails when trying to match a backreference to an unmatched group"
        assert_eq!(find("(?:(a)|b)\\1", "b"), None);
    }

    #[test]
    fn lookaround_does_not_consume() {
        assert_eq!(find("foo(?=bar)", "foobar"), Some((0, 3)));
        assert_eq!(find("foo(?!bar)", "foobar"), None);
        assert_eq!(find("(?<=foo)bar", "foobar"), Some((3, 6)));
        assert_eq!(find("(?<!foo)bar", "foobar"), None);
        assert_eq!(find("(?<!zap)bar", "foobar"), Some((3, 6)));
    }

    #[test]
    fn positive_lookahead_keeps_its_captures() {
        let re = Regex::new("(?=(a))", Flags::default()).expect("compiles");
        let caps = re.find_at("a", 0).expect("runs").expect("matches");
        assert_eq!(caps.group(1), Some((0, 1)));
    }

    #[test]
    fn atomic_groups_do_not_give_back() {
        assert_eq!(find("(?>a+)b", "aaab"), Some((0, 4)));
        // The atomic group takes every `a`, so there is none left for `ab`.
        assert_eq!(find("(?>a+)ab", "aaab"), None);
        // A possessive quantifier is the same thing spelled shorter.
        assert_eq!(find("a++ab", "aaab"), None);
    }

    #[test]
    fn an_empty_iteration_ends_the_loop_but_keeps_its_captures() {
        // `/(a*)*/ =~ ""` sets `$1` to "" in Ruby, not to nil, and terminates.
        let re = Regex::new("(a*)*", Flags::default()).expect("compiles");
        let caps = re.find_at("", 0).expect("runs").expect("matches");
        assert_eq!(caps.group(0), Some((0, 0)));
        assert_eq!(caps.group(1), Some((0, 0)));
    }

    #[test]
    fn case_folding_applies_to_literals_classes_and_ranges() {
        let flags = Flags {
            icase: true,
            ..Flags::default()
        };
        for pattern in ["foo", "[f]oo", "[a-z]oo"] {
            let re = Regex::new(pattern, flags).expect("compiles");
            assert!(re.is_match("FOO").expect("runs"), "{pattern} should fold");
        }
    }

    #[test]
    fn class_intersection_needs_both_halves() {
        assert_eq!(find("[a-z&&[^b]]", "b"), None);
        assert_eq!(find("[a-z&&[^b]]", "c"), Some((0, 1)));
    }

    #[test]
    fn options_round_trip_through_rubys_integer() {
        let flags = Flags {
            icase: true,
            dotall: true,
            extended: false,
        };
        assert_eq!(flags.to_options(), 1 | 4);
        assert_eq!(Flags::from_options(flags.to_options()), flags);
        assert_eq!(flags.to_letters(), "mi");
    }

    #[test]
    fn escape_quotes_what_ruby_quotes() {
        assert_eq!(escape("a.b"), "a\\.b");
        assert_eq!(escape("a b"), "a\\ b");
        assert_eq!(escape("1+1"), "1\\+1");
    }

    #[test]
    fn a_pathological_pattern_refuses_rather_than_hangs() {
        let re = Regex::new("(a+)+b", Flags::default()).expect("compiles");
        assert_eq!(re.is_match(&"a".repeat(60)), Err(Error::Budget));
    }

    #[test]
    fn unsupported_constructs_refuse_at_compile_time() {
        assert!(matches!(
            Regex::new("(?~foo)", Flags::default()),
            Err(Error::Unsupported(_))
        ));
    }
}

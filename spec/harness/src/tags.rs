//! `spec/tags/`, in mspec's format.
//!
//! A tag says "this example is not run, and here is why". It is the only
//! sanctioned way past a spec a slice will not fix — `CLAUDE.md` and
//! `docs/engine.md` both forbid marking one expected-to-fail in code — and it is
//! a debt rather than a result. `spec/tags/README.md` is the file a human reads.
//!
//! The format is mspec's, so these files keep working when mspec replaces this
//! harness ([#145](https://github.com/ar4mirez/spinel/issues/145)):
//!
//! ```text
//! fails(the reason):the example's full description
//! ```
//!
//! and the path is mspec's rewrite of the spec file's own:
//!
//! ```text
//! spec/ruby/language/regexp/empty_checks_spec.rb
//! spec/tags/language/regexp/empty_checks_tags.txt
//! ```
//!
//! What is Spinel's and not mspec's is that the reason is *required*. mspec
//! treats it as an optional comment and never writes one; a tag without one here
//! is an error, because a skip whose reason nobody wrote down is indistinguishable
//! from a spec quietly swept under the rug.

use std::path::{Component, Path, PathBuf};

/// The one tag name honoured here.
///
/// mspec also defines `critical`, `slow`, `unstable` and more. Accepting them
/// would mean a line that looks live and does nothing, so anything else is an
/// error rather than a no-op.
const TAG: &str = "fails";

/// One `fails(reason):description` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// Why this example is not run. Never empty: that is the point.
    pub reason: String,
    /// The full description mspec prints — every enclosing `describe`, then the
    /// `it`, joined by spaces. This is the key the example is matched on.
    pub description: String,
}

/// A spec file's tag file, read.
#[derive(Debug, Default)]
pub struct TagFile {
    /// Usable tags, in file order.
    pub tags: Vec<Tag>,
    /// Everything wrong with the file. Each one fails the run: a tag the reader
    /// cannot use is a skip that silently stopped happening, which is worse than
    /// no tag at all.
    pub problems: Vec<String>,
}

/// Where `spec_file`'s tags live under `tags_root`.
///
/// mspec's own rewrite, from `MSpec.tags_file`:
///
/// ```text
/// path/to/spec/class/method_spec.rb => path/to/spec/tags/class/method_tags.txt
/// ```
///
/// A file outside the corpus keeps only its name. That is what lets this
/// harness's own tests write a tag for a spec file in a temp directory; nothing
/// in `spec/ruby` reaches that branch.
#[must_use]
pub fn path_for(spec_file: &Path, tags_root: &Path) -> PathBuf {
    let relative = corpus_relative(spec_file);
    let mut path = tags_root.join(&relative);
    let name = relative
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let stem = name
        .strip_suffix("_spec.rb")
        .or_else(|| name.strip_suffix(".rb"))
        .unwrap_or(name);
    path.set_file_name(format!("{stem}_tags.txt"));
    path
}

/// The part of a spec file's path after the corpus root, or its file name when
/// it is not under one.
fn corpus_relative(spec_file: &Path) -> PathBuf {
    let parts: Vec<Component<'_>> = spec_file.components().collect();
    // The last `spec/ruby`, not the first: a checkout can live anywhere, and
    // `~/spec/ruby/spinel/spec/ruby/core` must resolve against the inner one.
    let corpus = parts
        .windows(2)
        .rposition(|pair| pair[0].as_os_str() == "spec" && pair[1].as_os_str() == "ruby");
    match corpus {
        Some(at) => parts[at + 2..].iter().collect(),
        None => PathBuf::from(spec_file.file_name().unwrap_or_default()),
    }
}

/// Read `spec_file`'s tag file, if it has one.
///
/// A missing file is not a problem: almost no spec has a tag, and the whole
/// point of this directory is to be empty one day. A file that exists and cannot
/// be read is a problem, because that one is a checkout in trouble.
#[must_use]
pub fn load(spec_file: &Path, tags_root: &Path) -> TagFile {
    let path = path_for(spec_file, tags_root);
    if !path.is_file() {
        return TagFile::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(err) => TagFile {
            tags: Vec::new(),
            problems: vec![format!("cannot be read: {err}")],
        },
    }
}

/// Every line of a tag file, as tags and complaints.
#[must_use]
pub fn parse(text: &str) -> TagFile {
    let mut file = TagFile::default();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Ok(tag) => file.tags.push(tag),
            Err(why) => file.problems.push(format!("line {}: {why}", number + 1)),
        }
    }
    file
}

/// One line, parsed the way mspec's `SpecTag` parses it.
///
/// mspec's regex, from `spec/mspec/lib/mspec/runner/tag.rb`:
///
/// ```text
/// /^([^()#:]+)(\(([^)]+)?\))?:(.*)$/
/// ```
///
/// Hand-written rather than pulled in with a regex crate, both to keep the
/// harness's dependencies to the ones it already has and because agreeing with
/// that expression character for character is the whole requirement — a
/// paraphrase that accepted one more line than mspec does would be a tag that
/// works here and vanishes after #145.
fn parse_line(line: &str) -> Result<Tag, String> {
    let end = line
        .find(['(', ')', '#', ':'])
        .ok_or("no `:`; a tag line is `fails(reason):description`")?;
    let (name, rest) = line.split_at(end);
    if name.is_empty() {
        return Err(format!(
            "starts with `{}`; a tag line is `fails(reason):description`",
            rest.chars().next().unwrap_or_default()
        ));
    }

    let (reason, rest) = match rest.strip_prefix('(') {
        Some(inside) => {
            let close = inside.find(')').ok_or(
                "unclosed `(`; a tag line is `fails(reason):description` and a reason \
                 may not contain a parenthesis",
            )?;
            let (reason, after) = inside.split_at(close);
            (Some(reason), &after[1..])
        }
        None => (None, rest),
    };
    // The reason lives in a field mspec closes at the first `)`, so a reason
    // holding one leaves the rest of the line where the `:` should be. mspec
    // drops such a line without a word and the example silently goes back to
    // failing; that is the failure this message exists to prevent.
    let description = rest.strip_prefix(':').ok_or(
        "expected `:` after the tag; a reason may not contain a parenthesis, because \
         mspec's tag parser stops at the first one and drops the line",
    )?;

    if name != TAG {
        return Err(format!(
            "unknown tag `{name}`; only `{TAG}` is honoured, and a tag this harness \
             ignores is a skip that silently stopped happening"
        ));
    }
    let reason = reason.map(str::trim).unwrap_or_default();
    if reason.is_empty() {
        return Err(format!(
            "no reason; write `{TAG}(why this is not run):{description}`. A skip \
             nobody wrote a reason for is a spec swept under the rug"
        ));
    }
    if reason.contains('(') {
        return Err(
            "a reason may not contain a parenthesis, because mspec's tag parser stops \
             at the first one and drops the line"
                .to_owned(),
        );
    }
    if description.trim().is_empty() {
        return Err("no description; a tag names the example it skips".to_owned());
    }

    Ok(Tag {
        reason: reason.to_owned(),
        description: description.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(line: &str) -> Result<Tag, String> {
        parse_line(line)
    }

    #[test]
    fn a_tag_carries_its_reason() {
        let tag = tag("fails(needs #14):Regexp foo bar").expect("a well-formed tag");
        assert_eq!(tag.reason, "needs #14");
        assert_eq!(tag.description, "Regexp foo bar");
    }

    #[test]
    fn a_tag_without_a_reason_is_an_error() {
        // mspec's own tag files look exactly like this, and mspec is happy with
        // them. Spinel is not: the reason is the whole point.
        let why = tag("fails:no reason at all").expect_err("a reason is required");
        assert!(why.contains("no reason"), "unhelpful: {why}");
    }

    #[test]
    fn a_reason_with_a_parenthesis_is_an_error() {
        // Measured against mspec: this line does not match its regex, so the tag
        // stops existing and the example quietly fails again. Loud here instead.
        // Three shapes, three arms: the `)` lands where the `:` should be...
        let why = tag("fails(m(*a) must expand first):desc").expect_err("parens are rejected");
        assert!(why.contains("parenthesis"), "unhelpful: {why}");
        // ...the `(` never closes...
        let why = tag("fails(unbalanced ( here):desc").expect_err("parens are rejected");
        assert!(why.contains("parenthesis"), "unhelpful: {why}");
        // ...or it does, and mspec would silently read half a reason.
        let why = tag("fails(a (b):desc").expect_err("parens are rejected");
        assert!(why.contains("parenthesis"), "unhelpful: {why}");
    }

    #[test]
    fn a_description_may_hold_anything() {
        // mspec's regex ends `:(.*)$`, so only the tag and reason are constrained.
        let tag = tag("fails(a colon: is fine):desc with (parens) and : colons")
            .expect("a well-formed tag");
        assert_eq!(tag.reason, "a colon: is fine");
        assert_eq!(tag.description, "desc with (parens) and : colons");
    }

    #[test]
    fn an_unknown_tag_is_an_error_rather_than_a_no_op() {
        let why = tag("slow(takes a while):desc").expect_err("only `fails` is honoured");
        assert!(why.contains("unknown tag `slow`"), "unhelpful: {why}");
    }

    #[test]
    fn a_comment_line_is_an_error() {
        // mspec drops these in silence. Somebody writing one has written
        // something no tool will ever read, so say so.
        let why = tag("# not a tag").expect_err("comments are not tags");
        assert!(!why.is_empty());
    }

    #[test]
    fn blank_lines_are_skipped_and_problems_carry_a_line_number() {
        let file = parse("\nfails(why):one\n\nfails:two\n");
        assert_eq!(file.tags.len(), 1);
        assert_eq!(file.tags[0].description, "one");
        assert_eq!(file.problems.len(), 1);
        assert!(
            file.problems[0].starts_with("line 4:"),
            "unhelpful: {}",
            file.problems[0]
        );
    }

    #[test]
    fn the_tags_path_mirrors_the_spec_path() {
        // mspec's documented rewrite, which is what makes these files survive
        // the harness being replaced by mspec.
        assert_eq!(
            path_for(
                Path::new("/w/spinel/spec/ruby/language/regexp/empty_checks_spec.rb"),
                Path::new("/w/spinel/spec/tags"),
            ),
            PathBuf::from("/w/spinel/spec/tags/language/regexp/empty_checks_tags.txt")
        );
    }

    #[test]
    fn a_spec_file_outside_the_corpus_keeps_only_its_name() {
        assert_eq!(
            path_for(Path::new("/tmp/x/two_spec.rb"), Path::new("/tags")),
            PathBuf::from("/tags/two_tags.txt")
        );
    }

    #[test]
    fn the_innermost_corpus_root_wins() {
        // A checkout can live anywhere, including under a directory that spells
        // `spec/ruby` itself.
        assert_eq!(
            path_for(
                Path::new("/spec/ruby/spinel/spec/ruby/core/array/pop_spec.rb"),
                Path::new("/t"),
            ),
            PathBuf::from("/t/core/array/pop_tags.txt")
        );
    }
}

//! The Gemfile edit behind `spinel add` (#85, docs/package-manager.md,
//! "9. `spinel add`"): the `gem` lines Bundler's `Injector` writes, spliced
//! into the Gemfile's own bytes at a place `spinel_ast` finds, so every other
//! byte of the file (its formatting, its comments, its quoting, its last
//! newline or the lack of one) stays as it was. The file is never printed
//! again from the tree.
//!
//! Where the lines go: a gem with no group, or with groups no top-level
//! `group ... do` block names exactly, is appended at the end of the file as
//! `bundle add` appends it (`Injector#append_to`: a newline, then the lines),
//! its groups written as a `group:` or `groups:` option. Groups a block names
//! exactly put the lines at the end of that block instead, indented as its
//! last statement is, and without the option, which the block supplies;
//! Bundler appends those too.
//!
//! [`remove`] is the inverse, for `spinel remove` (#86): the lines of each
//! `gem` statement taken out of those same bytes.

use spinel_ast::{BlockArg, Expr, ExprKind, StrPart};

/// A gem to add: `gem "name", "req"..., git: "url"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewGem {
    pub name: String,
    /// Each one `op version`, as `Gem::Requirement#as_list` writes it.
    pub requirements: Vec<String>,
    pub git: Option<String>,
}

/// `source` with a `gem` line for each of `gems`, in `groups` (none is the
/// default group).
pub(crate) fn add(source: &[u8], gems: &[NewGem], groups: &[String]) -> Vec<u8> {
    let newline: &[u8] = if first_line_ends_crlf(source) {
        b"\r\n"
    } else {
        b"\n"
    };
    let parsed = spinel_parse::parse(source);
    let block = if groups.is_empty() || !parsed.is_ok() {
        None
    } else {
        parsed
            .program
            .body
            .iter()
            .find_map(|expr| group_block_end(source, expr, groups))
    };
    let mut out = Vec::with_capacity(source.len() + 64 * gems.len());
    match block {
        Some((at, indent)) => {
            out.extend_from_slice(&source[..at]);
            for gem in gems {
                out.extend_from_slice(&indent);
                out.extend_from_slice(line(gem, &[]).as_bytes());
                out.extend_from_slice(newline);
            }
            out.extend_from_slice(&source[at..]);
        }
        None => {
            // `f.puts; f.puts lines`: a newline, which ends a last line that
            // had none and is a blank line otherwise, then the lines.
            let last = parsed
                .program
                .body
                .last()
                .map_or(0, |e| e.span.end as usize);
            let at = if parsed.is_ok() {
                data_start(source, last)
            } else {
                source.len()
            };
            out.extend_from_slice(&source[..at]);
            out.extend_from_slice(newline);
            for gem in gems {
                out.extend_from_slice(line(gem, groups).as_bytes());
                out.extend_from_slice(newline);
            }
            out.extend_from_slice(&source[at..]);
        }
    }
    out
}

/// `Injector#build_gem_lines` for one gem: the name and each requirement
/// `dump`ed, then the groups, then `git:`.
pub(crate) fn line(gem: &NewGem, groups: &[String]) -> String {
    let mut line = format!("gem {}", dump(&gem.name));
    for requirement in &gem.requirements {
        line.push_str(", ");
        line.push_str(&dump(requirement));
    }
    match groups {
        [] => {}
        [group] => line.push_str(&format!(", group: {}", symbol(group))),
        groups => {
            let list: Vec<String> = groups.iter().map(|g| symbol(g)).collect();
            line.push_str(&format!(", groups: [{}]", list.join(", ")));
        }
    }
    if let Some(git) = &gem.git {
        line.push_str(&format!(", git: {}", dump(git)));
    }
    line
}

/// `String#dump`, for the printable ASCII a gem name, a requirement or a URL
/// is written in; anything else is a `\x` escape, which reads back the same.
fn dump(s: &str) -> String {
    dump_bytes(s.as_bytes())
}

/// A double-quoted Ruby literal that reads back as `bytes`.
pub(crate) fn dump_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() + 2);
    out.push('"');
    for &b in bytes {
        match b {
            b'"' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            // `#{`, `#$` and `#@` would interpolate.
            b'#' => out.push_str("\\#"),
            b' '..=b'~' => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02X}")),
        }
    }
    out.push('"');
    out
}

/// `Symbol#inspect` of a group name: `:dev`, or `:"my group"` for one that
/// is not an identifier.
fn symbol(name: &str) -> String {
    let mut chars = name.chars();
    let plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && {
            let rest = &name[1..];
            let body = rest.strip_suffix(['?', '!']).unwrap_or(rest);
            body.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
    if plain {
        format!(":{name}")
    } else {
        format!(":{}", dump(name))
    }
}

/// Whether the file's lines end in `\r\n`, as its first one does.
fn first_line_ends_crlf(source: &[u8]) -> bool {
    source
        .iter()
        .position(|&b| b == b'\n')
        .is_some_and(|at| at > 0 && source[at - 1] == b'\r')
}

/// Where a file's code ends: the start of an `__END__` line after the last
/// statement, which begins the `DATA` the Gemfile does not evaluate, else
/// the end of the file.
fn data_start(source: &[u8], after: usize) -> usize {
    let mut at = after.min(source.len());
    // From the start of the line after `after`.
    match source[at..].iter().position(|&b| b == b'\n') {
        Some(nl) => at += nl + 1,
        None => return source.len(),
    }
    while at < source.len() {
        let end = source[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(source.len(), |nl| at + nl);
        let line = &source[at..end];
        if line.strip_suffix(b"\r").unwrap_or(line) == b"__END__" {
            return at;
        }
        at = end + 1;
    }
    source.len()
}

/// For a top-level `group :a, :b do ... end` naming exactly `groups`: where
/// a line added at the end of its body starts (the start of the line its
/// closing `end` is on), and the indentation to give it. `None` for any
/// other statement, and for a block whose `end` shares a line with code.
fn group_block_end(source: &[u8], expr: &Expr, groups: &[String]) -> Option<(usize, Vec<u8>)> {
    let ExprKind::Call(call) = &expr.kind else {
        return None;
    };
    if call.receiver.is_some() || &*call.name != "group" {
        return None;
    }
    let Some(BlockArg::Block(block)) = &call.block else {
        return None;
    };
    let mut named: Vec<String> = Vec::new();
    for arg in &call.args {
        let (ExprKind::Sym(lit) | ExprKind::Str(lit)) = &arg.kind else {
            return None;
        };
        let [StrPart::Bytes(bytes)] = lit.parts.as_slice() else {
            return None;
        };
        named.push(String::from_utf8(bytes.to_vec()).ok()?);
    }
    let mut want: Vec<&str> = groups.iter().map(String::as_str).collect();
    let mut have: Vec<&str> = named.iter().map(String::as_str).collect();
    want.sort_unstable();
    want.dedup();
    have.sort_unstable();
    have.dedup();
    if want != have {
        return None;
    }

    let end = expr.span.end as usize;
    let closing = if source[..end].ends_with(b"end") {
        end - 3
    } else {
        end.checked_sub(1)?
    };
    let line_start = line_start(source, closing);
    // The closing keyword alone on its line, below the `do`.
    if line_start <= expr.span.start as usize
        || !source[line_start..closing]
            .iter()
            .all(|&b| b == b' ' || b == b'\t')
    {
        return None;
    }
    let indent = match block.body.last() {
        Some(last)
            if line_start_of(source, last.span.start) > line_start_of(source, expr.span.start) =>
        {
            leading_space(source, line_start_of(source, last.span.start))
        }
        _ => {
            let mut indent = leading_space(source, line_start_of(source, expr.span.start));
            indent.extend_from_slice(b"  ");
            indent
        }
    };
    Some((line_start, indent))
}

fn line_start(source: &[u8], at: usize) -> usize {
    source[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |nl| nl + 1)
}

fn line_start_of(source: &[u8], at: u32) -> usize {
    line_start(source, at as usize)
}

fn leading_space(source: &[u8], start: usize) -> Vec<u8> {
    source[start..]
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .copied()
        .collect()
}

/// The blocks `bundle remove` takes out once a removal leaves them empty
/// (`Injector#remove_nested_blocks`).
const EMPTIED_BLOCKS: [&str; 4] = ["group", "source", "env", "install_if"];

/// The Gemfile edit behind `spinel remove` (#86): `source` without each
/// `gem` statement naming one of `names`, the whole lines it is on taken
/// out (with a comment after it, and every line of a call written across
/// several), and without a `group`, `source`, `env` or `install_if` block
/// that removal leaves empty, with a blank line right after its `end`, as
/// `bundle remove` takes those out. Every other byte stays. A statement that shares a line with other code is left
/// where it is; the evaluation after says it could not be removed.
pub(crate) fn remove(source: &[u8], names: &[String]) -> Vec<u8> {
    let parsed = spinel_parse::parse(source);
    if !parsed.is_ok() {
        return source.to_vec();
    }
    let mut ranges = Vec::new();
    removals(source, &parsed.program.body, names, &mut ranges);
    ranges.sort_unstable_by_key(|r| r.start);
    let mut out = Vec::with_capacity(source.len());
    let mut at = 0;
    for range in ranges {
        if range.start >= at {
            out.extend_from_slice(&source[at..range.start]);
            at = range.end;
        }
    }
    out.extend_from_slice(&source[at..]);
    out
}

/// The byte ranges to take out of `body`'s statements, into `out`. Whether
/// every statement of `body` goes.
fn removals(
    source: &[u8],
    body: &[Expr],
    names: &[String],
    out: &mut Vec<std::ops::Range<usize>>,
) -> bool {
    let mut all = !body.is_empty();
    for expr in body {
        if names_gem(expr, names) || modifier_of_gem(expr, names) {
            match whole_lines(source, expr.span.start as usize, expr.span.end as usize) {
                Some(range) => out.push(range),
                None => all = false,
            }
            continue;
        }
        let mut gone = false;
        match &expr.kind {
            ExprKind::Call(call) => {
                let Some(BlockArg::Block(block)) = &call.block else {
                    continue;
                };
                let mut inner = Vec::new();
                let emptied = removals(source, &block.body, names, &mut inner);
                let whole = if emptied
                    && call.receiver.is_none()
                    && EMPTIED_BLOCKS.contains(&&*call.name)
                {
                    emptied_block(source, expr, call, &inner)
                } else {
                    None
                };
                match whole {
                    Some(range) => {
                        out.push(range);
                        gone = true;
                    }
                    None => out.extend(inner),
                }
            }
            ExprKind::If(branch) => {
                removals(source, &branch.then_body, names, out);
                if let Some(other) = &branch.else_body {
                    removals(source, other, names, out);
                }
            }
            _ => {}
        }
        all &= gone;
    }
    all
}

/// `gem "name", ...` for one of `names`, written as a String or Symbol.
fn names_gem(expr: &Expr, names: &[String]) -> bool {
    let ExprKind::Call(call) = &expr.kind else {
        return false;
    };
    if call.receiver.is_some() || &*call.name != "gem" {
        return false;
    }
    let Some(first) = call.args.first() else {
        return false;
    };
    let (ExprKind::Str(lit) | ExprKind::Sym(lit)) = &first.kind else {
        return false;
    };
    let [StrPart::Bytes(bytes)] = lit.parts.as_slice() else {
        return false;
    };
    names.iter().any(|n| n.as_bytes() == &bytes[..])
}

/// `gem "name" if cond` (or `unless`): the modifier form, which `bundle
/// remove` takes out with its line.
fn modifier_of_gem(expr: &Expr, names: &[String]) -> bool {
    let ExprKind::If(branch) = &expr.kind else {
        return false;
    };
    match (branch.then_body.as_slice(), &branch.else_body) {
        ([gem], None) => gem.span.start < branch.predicate.span.start && names_gem(gem, names),
        _ => false,
    }
}

/// The whole lines from `start` to `end`, through the newline after `end`,
/// when nothing but space comes before `start` on its line and nothing but
/// space and a comment after `end` on its.
fn whole_lines(source: &[u8], start: usize, end: usize) -> Option<std::ops::Range<usize>> {
    let from = line_start(source, start);
    if !source[from..start].iter().all(|&b| b == b' ' || b == b'\t') {
        return None;
    }
    let to = line_end(source, end);
    let rest = &source[end..to];
    let rest = rest.strip_suffix(b"\n").unwrap_or(rest);
    let rest = rest.strip_suffix(b"\r").unwrap_or(rest);
    let code = rest
        .iter()
        .position(|&b| b == b'#')
        .map_or(rest, |hash| &rest[..hash]);
    code.iter()
        .all(|&b| b == b' ' || b == b'\t')
        .then_some(from..to)
}

/// The lines of a `name ... do ... end` block whose statements `inner` takes
/// out, when what is left between its `do` line and its `end` line is blank
/// and the `end` is alone on its line, as `remove_nested_blocks` finds them.
fn emptied_block(
    source: &[u8],
    expr: &Expr,
    call: &spinel_ast::Call,
    inner: &[std::ops::Range<usize>],
) -> Option<std::ops::Range<usize>> {
    let end = expr.span.end as usize;
    if !source[..end].ends_with(b"end") {
        return None;
    }
    let end_line = line_start(source, end - 3);
    // The `do`: after the arguments, the last thing on its line but for a
    // comment.
    let after_args = call.args.last().map_or(call.name_span.end, |a| a.span.end) as usize;
    let header = line_end(source, after_args);
    let opening = &source[after_args..header];
    let opening = std::str::from_utf8(opening).ok()?;
    let opening = opening.trim_start_matches([' ', '\t', ')']);
    let opening = opening.strip_prefix("do")?;
    let opening = opening.split('#').next().unwrap_or("");
    if !opening.trim().is_empty() || header > end_line {
        return None;
    }
    let mut at = header;
    let mut sorted: Vec<_> = inner.to_vec();
    sorted.sort_unstable_by_key(|r| r.start);
    for range in &sorted {
        if !source[at..range.start.max(at)]
            .iter()
            .all(u8::is_ascii_whitespace)
        {
            return None;
        }
        at = at.max(range.end);
    }
    if at > end_line || !source[at..end_line].iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let lines = whole_lines(source, expr.span.start as usize, end).filter(|_| {
        let last = &source[end_line..line_end(source, end)];
        last.iter()
            .filter(|b| !b.is_ascii_whitespace())
            .eq(b"end".iter())
    })?;
    // A blank line (exactly "\n") right after the `end` goes with it, as
    // `bundle remove` glues a lone "\n" onto the line before it before
    // `remove_nested_blocks` drops that line.
    let to = lines.end + usize::from(source[lines.end..].starts_with(b"\n"));
    Some(lines.start..to)
}

/// The end of the line `at` is on, past its newline (or the end of the
/// file).
fn line_end(source: &[u8], at: usize) -> usize {
    source[at..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(source.len(), |nl| at + nl + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gem(name: &str, requirement: &str) -> NewGem {
        NewGem {
            name: name.to_owned(),
            requirements: vec![requirement.to_owned()],
            git: None,
        }
    }

    fn added(source: &str, gems: &[NewGem], groups: &[&str]) -> String {
        let groups: Vec<String> = groups.iter().map(|g| (*g).to_owned()).collect();
        String::from_utf8(add(source.as_bytes(), gems, &groups)).unwrap()
    }

    #[test]
    fn a_gem_is_appended_as_bundle_add_appends_it() {
        // Each of these is what `bundle add` 4.0.20 wrote.
        assert_eq!(
            added(
                "source \"https://rubygems.org\"\n\n# Tools\ngem 'rake' # build\n",
                &[gem("sinatra", "~> 4.2")],
                &[]
            ),
            "source \"https://rubygems.org\"\n\n# Tools\ngem 'rake' # build\n\ngem \"sinatra\", \"~> 4.2\"\n"
        );
        assert_eq!(
            added(
                "source \"https://rubygems.org\"\ngem \"rake\"",
                &[gem("sinatra", "~> 4.2")],
                &[]
            ),
            "source \"https://rubygems.org\"\ngem \"rake\"\ngem \"sinatra\", \"~> 4.2\"\n"
        );
        assert_eq!(
            added("source \"x\"\n", &[gem("rake", "~> 13.4")], &["test"]),
            "source \"x\"\n\ngem \"rake\", \"~> 13.4\", group: :test\n"
        );
        assert_eq!(
            added(
                "source \"x\"\n",
                &[gem("rake", "~> 13.4")],
                &["dev", "test"]
            ),
            "source \"x\"\n\ngem \"rake\", \"~> 13.4\", groups: [:dev, :test]\n"
        );
    }

    #[test]
    fn line_shapes_follow_build_gem_lines() {
        let rake = NewGem {
            name: "rake".to_owned(),
            requirements: vec!["> 1".to_owned(), "< 14".to_owned()],
            git: Some("https://example.com/rake.git".to_owned()),
        };
        assert_eq!(
            line(&rake, &["my-group".to_owned()]),
            "gem \"rake\", \"> 1\", \"< 14\", group: :\"my-group\", git: \"https://example.com/rake.git\""
        );
        assert_eq!(symbol("dev"), ":dev");
        assert_eq!(symbol("ok?"), ":ok?");
        assert_eq!(symbol("2fa"), ":\"2fa\"");
        assert_eq!(dump("a\"#{b}\\"), "\"a\\\"\\#{b}\\\\\"");
    }

    #[test]
    fn a_group_block_takes_the_line_at_its_end() {
        let source = "source \"x\"\n\ngroup :development, :test do\n  gem \"rake\"\nend\n\ngroup :test do\n    gem \"minitest\" # tests\n    # gem \"later\"\nend\n";
        assert_eq!(
            added(source, &[gem("rspec", "~> 3.13")], &["test"]),
            "source \"x\"\n\ngroup :development, :test do\n  gem \"rake\"\nend\n\ngroup :test do\n    gem \"minitest\" # tests\n    # gem \"later\"\n    gem \"rspec\", \"~> 3.13\"\nend\n"
        );
        // The same groups in another order, written as strings.
        assert_eq!(
            added(source, &[gem("rspec", "~> 3.13")], &["test", "development"]),
            "source \"x\"\n\ngroup :development, :test do\n  gem \"rake\"\n  gem \"rspec\", \"~> 3.13\"\nend\n\ngroup :test do\n    gem \"minitest\" # tests\n    # gem \"later\"\nend\n"
        );
        // An empty block, indented itself.
        assert_eq!(
            added(
                "if true\n  group \"dev\" do\n  end\nend\n  group \"dev\" do\n    # none yet\n  end\n",
                &[gem("pry", "~> 0.15.2"), gem("irb", "~> 1.15")],
                &["dev"]
            ),
            "if true\n  group \"dev\" do\n  end\nend\n  group \"dev\" do\n    # none yet\n    gem \"pry\", \"~> 0.15.2\"\n    gem \"irb\", \"~> 1.15\"\n  end\n"
        );
    }

    #[test]
    fn a_block_it_cannot_add_to_is_left_alone() {
        // Its `end` on a line with code, its groups not the ones asked for,
        // or options of its own: the line is appended, with its group.
        for source in [
            "group :dev do gem \"a\" end\n",
            "group :dev do\n  gem \"a\" end\n",
            "group :dev, :test do\nend\n",
            "group :dev, optional: true do\nend\n",
        ] {
            assert_eq!(
                added(source, &[gem("b", "~> 1.0")], &["dev"]),
                format!("{source}\ngem \"b\", \"~> 1.0\", group: :dev\n"),
                "{source:?}"
            );
        }
    }

    #[test]
    fn crlf_and_data_are_kept() {
        assert_eq!(
            added("source \"x\"\r\ngem \"a\"\r\n", &[gem("b", "~> 1.0")], &[]),
            "source \"x\"\r\ngem \"a\"\r\n\r\ngem \"b\", \"~> 1.0\"\r\n"
        );
        assert_eq!(
            added(
                "source \"x\"\n# __END__\n__END__\ngem \"not code\"\n",
                &[gem("b", "~> 1.0")],
                &[]
            ),
            "source \"x\"\n# __END__\n\ngem \"b\", \"~> 1.0\"\n__END__\ngem \"not code\"\n"
        );
        assert_eq!(
            added("", &[gem("b", "~> 1.0")], &[]),
            "\ngem \"b\", \"~> 1.0\"\n"
        );
    }

    fn removed(source: &str, names: &[&str]) -> String {
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        String::from_utf8(remove(source.as_bytes(), &names)).unwrap()
    }

    #[test]
    fn a_gem_line_goes_and_nothing_else_does() {
        let source = "# frozen_string_literal: true\nsource  'x'   # index\n\n# Tools\ngem 'rake',   '~> 13.0' # tasks\n\ngem \"tilt\"";
        assert_eq!(
            removed(source, &["rake"]),
            "# frozen_string_literal: true\nsource  'x'   # index\n\n# Tools\n\ngem \"tilt\""
        );
        // The last line, without its newline; a commented-out one stays.
        assert_eq!(
            removed("# gem \"tilt\"\ngem \"rake\"\ngem \"tilt\"", &["tilt"]),
            "# gem \"tilt\"\ngem \"rake\"\n"
        );
        // Parentheses, a Symbol, several lines, `\r\n`, a modifier.
        assert_eq!(
            removed(
                "gem(\"a\", \"~> 1\")\r\ngem :b\r\ngem \"c\",\r\n  \"~> 2\",\r\n  require: false\r\ngem \"d\" if ENV[\"D\"]\r\nkeep\r\n",
                &["a", "b", "c", "d"]
            ),
            "keep\r\n"
        );
    }

    #[test]
    fn a_gem_sharing_its_line_stays() {
        for source in [
            "gem \"a\"; gem \"b\"\n",
            "x = 1; gem \"a\"\n",
            "group :dev do gem \"a\" end\n",
        ] {
            assert_eq!(removed(source, &["a"]), source, "{source:?}");
        }
        // `if` around it stays; the line inside goes.
        assert_eq!(
            removed(
                "if true\n  gem \"a\"\nelse\n  gem \"a\", \"1\"\nend\n",
                &["a"]
            ),
            "if true\nelse\nend\n"
        );
    }

    #[test]
    fn a_block_left_empty_goes_as_bundle_remove_takes_it() {
        // What `bundle remove minitest tilt` 4.0.20 left of the same file.
        assert_eq!(
            removed(
                "source \"x\"\n\n# gem \"tilt\" is commented\ngem \"rake\" # tasks\n\ngroup :test do\n  group :ci do\n    gem \"minitest\",\n      \"~> 6.0\",\n      require: false\n  end\nend\ngem(\"tilt\", \"~> 2.0\")\ngem \"x\", git: \"https://e/x\" if false\n",
                &["minitest", "tilt"]
            ),
            "source \"x\"\n\n# gem \"tilt\" is commented\ngem \"rake\" # tasks\n\ngem \"x\", git: \"https://e/x\" if false\n"
        );
        // The blank line after its `end` goes with it, nested or not; one
        // after a gem line stays. What `bundle remove tilt rake` 4.0.20
        // left of the same file, but for the last newline.
        assert_eq!(
            removed(
                "# my app\nsource \"x\"\n\ngem \"rack-session\"\n\ngroup :development, :test do\n  gem \"tilt\"\nend\n\ngroup :test do\n  # keep me\n  gem \"rake\"\nend\n\ngroup :a do\n  group :b do\n    gem \"tilt\"\n  end\n\n  gem \"x\"\nend\n",
                &["tilt", "rake"]
            ),
            "# my app\nsource \"x\"\n\ngem \"rack-session\"\n\ngroup :test do\n  # keep me\nend\n\ngroup :a do\n  gem \"x\"\nend\n"
        );
        assert_eq!(removed("gem \"a\"\n\ngem \"b\"\n", &["a"]), "\ngem \"b\"\n");
        // Blank lines inside go with it, and a comment after its `do`.
        assert_eq!(
            removed(
                "group :dev do # tools\n\n  gem \"a\"\n\nend\nkeep\n",
                &["a"]
            ),
            "keep\n"
        );
        // A comment inside, a gem left, a comment after `end`, a block
        // `bundle remove` keeps, or one already empty: the block stays.
        for (source, left) in [
            (
                "group :dev do\n  # tools\n  gem \"a\"\nend\n",
                "group :dev do\n  # tools\nend\n",
            ),
            (
                "group :dev do\n  gem \"a\"\n  gem \"b\"\nend\n",
                "group :dev do\n  gem \"b\"\nend\n",
            ),
            (
                "group :dev do\n  gem \"a\"\nend # dev\n",
                "group :dev do\nend # dev\n",
            ),
            (
                "platforms :ruby do\n  gem \"a\"\nend\n",
                "platforms :ruby do\nend\n",
            ),
            ("group :test do\nend\ngem \"a\"\n", "group :test do\nend\n"),
        ] {
            assert_eq!(removed(source, &["a"]), left, "{source:?}");
        }
        // `source`, `env` and `install_if` blocks go as `group` ones do.
        assert_eq!(
            removed(
                "source \"https://e\" do\n  gem \"a\"\nend\nenv \"X\" do\n  gem \"a\"\nend\ninstall_if -> { true } do\n  gem \"a\"\nend\n",
                &["a"]
            ),
            ""
        );
    }
}

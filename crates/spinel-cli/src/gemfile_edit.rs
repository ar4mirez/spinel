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
}

//! `ruby`'s own command line (#62): `spinel -e CODE`, `spinel -w -Ilib
//! script.rb args`, and the rest of the flags `ruby.c`'s `proc_options`
//! takes, so that scripts and gems which shell out to `RUBY_EXE` keep
//! working.
//!
//! Parsing is here; meaning is Ruby. Each flag becomes a line of a prelude
//! that runs before the program — `$VERBOSE = true` for `-w`, `require` for
//! `-r` — and `-n`, `-p`, `-a` and `-l` wrap the program in the loop `ruby`
//! wraps it in.

use std::fmt::Write as _;

/// What the command line asked for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RubyArgs {
    /// `-e` lines, joined by newlines; when there are any, there is no script.
    pub code: Option<String>,
    /// Where each `-e` line was: the word, and the byte it starts at. The
    /// words reach the parser as UTF-8, and a line in another encoding is
    /// taken from the word's own bytes by these.
    pub code_at: Vec<(usize, usize)>,
    /// The script, or `-` for standard input.
    pub script: Option<String>,
    /// What the program sees as `ARGV`.
    pub argv: Vec<String>,
    pub requires: Vec<String>,
    pub includes: Vec<String>,
    /// `$VERBOSE`: `-W0` nil, `-W1` false, `-w`/`-W`/`-W2`/`-v`/`-d` true.
    pub verbose: Option<Verbose>,
    /// `-W:no-deprecated` and the like: `[category, enabled]`.
    pub warning_categories: Vec<(String, bool)>,
    pub debug: bool,
    pub loop_mode: Option<LoopMode>,
    pub autosplit: bool,
    pub line_ending: bool,
    pub split_pattern: Option<String>,
    /// `-0`'s octal digits as written: none is NUL, `0` paragraph mode,
    /// anything past a byte (`777`) the whole file.
    pub record_separator: Option<String>,
    pub switches: bool,
    pub chdir: Option<String>,
    /// `-x`: skip to the `#!ruby` line, after `chdir` into the directory given.
    pub skip_to_ruby: Option<Option<String>>,
    /// `-S`: look for the script on `RUBYPATH`, then `PATH`.
    pub search_path: bool,
    pub syntax_only: bool,
    /// `-v`: the version banner first, and with no program nothing else.
    pub print_version: bool,
    pub version_only: bool,
    pub external_encoding: Option<String>,
    pub internal_encoding: Option<String>,
    /// `-K`: the main script's source encoding, when it has no magic comment.
    pub source_encoding: Option<String>,
    pub backtrace_limit: Option<String>,
    /// `--enable`/`--disable` features, in the order given: `(name, on)`.
    pub features: Vec<(String, bool)>,
    /// Warnings `ruby` prints about the command line, `warning:` and all.
    pub warnings: Vec<String>,
    /// `RUBYLIB`'s directories, which go after `-I`'s.
    pub library_path: Vec<String>,
    pub help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbose {
    Nil,
    False,
    True,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    /// `-n`: `while gets; ...; end`.
    Read,
    /// `-p`: the same, printing `$_` after each.
    Print,
}

/// The features `--enable` and `--disable` know, as `ruby` lists them.
const FEATURES: &[&str] = &[
    "gems",
    "error_highlight",
    "did_you_mean",
    "syntax_suggest",
    "rubyopt",
    "frozen_string_literal",
    "yjit",
    "zjit",
];

/// Whether a command line is a `ruby` one: a flag that is not one of
/// `spinel`'s own, or `-` for a script on standard input.
pub fn is_ruby_invocation(first: &str) -> bool {
    first.starts_with('-') && !matches!(first, "-h" | "--help" | "-V")
}

/// `ruby.c`'s `proc_options`, over everything after the program name.
pub fn parse(args: &[String]) -> Result<RubyArgs, String> {
    let mut out = RubyArgs::default();
    let mut code: Vec<String> = Vec::new();
    let i = options(args, &mut out, &mut code, false)?;
    let mut i = i;
    if !code.is_empty() {
        out.code = Some(code.join("\n"));
    } else if i < args.len() {
        out.script = Some(args[i].clone());
        i += 1;
    } else if out.print_version {
        out.version_only = true;
    } else {
        out.script = Some("-".to_owned());
    }
    out.argv = args[i..].to_vec();
    Ok(out)
}

/// `RUBYOPT`'s switches, after the command line's: its `-I` and `-r` come
/// after theirs. Only the switches that change how a program runs are
/// allowed, and a word without its `-` is given one. Nothing is done when
/// `--disable=rubyopt` (or `all`) was given.
pub fn apply_rubyopt(out: &mut RubyArgs, value: &str) -> Result<(), String> {
    if !feature_enabled(out, "rubyopt", true) {
        return Ok(());
    }
    let words: Vec<String> = value
        .split_whitespace()
        .map(|w| {
            if w.starts_with('-') {
                w.to_owned()
            } else {
                format!("-{w}")
            }
        })
        .collect();
    let mut code = Vec::new();
    let mut extra = RubyArgs::default();
    options(&words, &mut extra, &mut code, true)?;
    out.includes.extend(extra.includes);
    out.requires.extend(extra.requires);
    out.verbose = out.verbose.or(extra.verbose);
    out.warning_categories.extend(extra.warning_categories);
    out.debug |= extra.debug;
    if extra.print_version && !out.print_version {
        out.print_version = true;
    }
    out.external_encoding = out.external_encoding.take().or(extra.external_encoding);
    if extra.internal_encoding.is_some() && out.internal_encoding.is_none() {
        out.internal_encoding = extra.internal_encoding;
    }
    out.source_encoding = out.source_encoding.take().or(extra.source_encoding);
    out.features.extend(extra.features);
    out.warnings.extend(extra.warnings);
    Ok(())
}

/// Whether a feature is on: the last `--enable`/`--disable` naming it, or
/// `all`, wins.
pub fn feature_enabled(args: &RubyArgs, feature: &str, default: bool) -> bool {
    feature_setting(args, feature).unwrap_or(default)
}

/// [`feature_enabled`], with `None` when no flag names the feature.
pub fn feature_setting(args: &RubyArgs, feature: &str) -> Option<bool> {
    let mut on = None;
    for (name, enabled) in &args.features {
        if name == feature || name == "all" {
            on = Some(*enabled);
        }
    }
    on
}

/// The switches in `args`, up to the script; the index where they end.
/// `rubyopt` refuses the ones `RUBYOPT` may not carry.
fn options(
    args: &[String],
    out: &mut RubyArgs,
    code: &mut Vec<String>,
    rubyopt: bool,
) -> Result<usize, String> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let name = long.split(['=']).next().unwrap_or("");
            if rubyopt
                && ![
                    "debug",
                    "verbose",
                    "encoding",
                    "external-encoding",
                    "internal-encoding",
                    "backtrace-limit",
                ]
                .contains(&name)
                && !name.starts_with("disable")
                && !name.starts_with("enable")
                && !name.starts_with("jit")
                && !name.starts_with("yjit")
            {
                return Err(format!("invalid switch in RUBYOPT: --{name}"));
            }
            let next = args.get(i + 1);
            if parse_long(long, next, out)? {
                i += 1;
            }
            i += 1;
            continue;
        }
        // Short flags combine, `-wn`; one that takes a value takes the rest
        // of the word, or else the next word where `ruby` allows it.
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut j = 0;
        while j < chars.len() {
            let c = chars[j];
            if rubyopt && !"dEIKrUvwWT".contains(c) {
                return Err(format!("invalid switch in RUBYOPT: -{c}"));
            }
            let rest: String = chars[j + 1..].iter().collect();
            match c {
                'a' => out.autosplit = true,
                'c' => out.syntax_only = true,
                'd' => {
                    out.debug = true;
                    out.verbose = Some(Verbose::True);
                }
                'l' => out.line_ending = true,
                'n' => {
                    if out.loop_mode.is_none() {
                        out.loop_mode = Some(LoopMode::Read);
                    }
                }
                'p' => out.loop_mode = Some(LoopMode::Print),
                's' => out.switches = true,
                'S' => out.search_path = true,
                'y' => {}
                'v' => {
                    out.print_version = true;
                    out.verbose = Some(Verbose::True);
                }
                'w' => out.verbose = Some(Verbose::True),
                'h' => out.help = true,
                'W' => {
                    if let Some(category) = rest.strip_prefix(':') {
                        let (name, enabled) = match category.strip_prefix("no-") {
                            Some(name) => (name, false),
                            None => (category, true),
                        };
                        out.warning_categories.push((name.to_owned(), enabled));
                    } else {
                        out.verbose = Some(match rest.as_str() {
                            "0" => Verbose::Nil,
                            "1" => Verbose::False,
                            _ => Verbose::True,
                        });
                    }
                    break;
                }
                'e' | 'r' | 'I' | 'C' | 'X' | 'E' => {
                    if c == 'e' && !rubyopt {
                        // The flags before `e` are ASCII, so its value starts
                        // that many bytes in, after the `-`.
                        out.code_at.push(if rest.is_empty() {
                            (i + 1, 0)
                        } else {
                            (i, j + 2)
                        });
                    }
                    let value = if !rest.is_empty() {
                        rest
                    } else {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or_else(|| format!("no code specified for -{c}"))?
                    };
                    match c {
                        'e' => code.push(value),
                        'r' => out.requires.push(value),
                        'I' => out.includes.push(value),
                        'C' | 'X' => out.chdir = Some(value),
                        _ => set_encodings(&value, "-E", out)?,
                    }
                    break;
                }
                'x' => {
                    out.skip_to_ruby = Some((!rest.is_empty()).then_some(rest));
                    break;
                }
                'F' => {
                    out.split_pattern = Some(rest);
                    break;
                }
                '0' => {
                    let digits: String = rest.chars().take_while(|d| d.is_digit(8)).collect();
                    out.record_separator = Some(digits.clone());
                    j += 1 + digits.len();
                    continue;
                }
                'K' => {
                    // The Ruby 1.8 codes: the main script's source encoding,
                    // and the default external encoding with it.
                    let name = match rest.chars().next() {
                        Some('a' | 'A' | 'n' | 'N') => Some("ASCII-8BIT"),
                        Some('e' | 'E') => Some("EUC-JP"),
                        Some('s' | 'S') => Some("Windows-31J"),
                        Some('u' | 'U') => Some("UTF-8"),
                        _ => None,
                    };
                    if let Some(name) = name {
                        out.source_encoding = Some(name.to_owned());
                        out.external_encoding = Some(name.to_owned());
                    }
                    j += 2;
                    continue;
                }
                'U' => set_internal(out, "UTF-8")?,
                'T' => {
                    // `$SAFE` is gone; `-T` and its level are accepted.
                    break;
                }
                _ => {
                    return Err(format!(
                        "invalid option -{c}  (-h will show valid options; `spinel --help` for its own commands)"
                    ));
                }
            }
            j += 1;
        }
        i += 1;
    }
    Ok(i)
}

/// One `--name[=value]`. Answers whether it took `next` as its value.
fn parse_long(long: &str, next: Option<&String>, out: &mut RubyArgs) -> Result<bool, String> {
    let (name, value) = match long.split_once('=') {
        Some((name, value)) => (name, Some(value.to_owned())),
        None => (long, None),
    };
    // The three encoding switches take their value as the next word too.
    let mut took_next = false;
    let mut valued = || -> Result<String, String> {
        if let Some(value) = &value {
            return Ok(value.clone());
        }
        took_next = true;
        next.cloned()
            .ok_or_else(|| format!("missing argument for --{name}"))
    };
    match name {
        "version" => {
            out.print_version = true;
            out.version_only = true;
        }
        "verbose" => out.verbose = Some(Verbose::True),
        "debug" => {
            out.debug = true;
            out.verbose = Some(Verbose::True);
        }
        "help" => out.help = true,
        "encoding" => {
            let value = valued()?;
            set_encodings(&value, "--encoding", out)?;
        }
        "external-encoding" => out.external_encoding = Some(valued()?),
        "internal-encoding" => {
            let value = valued()?;
            set_internal(out, &value)?;
        }
        "backtrace-limit" => out.backtrace_limit = value.clone(),
        _ if name.starts_with("enable") || name.starts_with("disable") => {
            let on = name.starts_with("enable");
            let verb = if on { "enable" } else { "disable" };
            // `--enable=a,b`, `--enable-a`, and `--enable a`.
            let list = match value.clone() {
                Some(list) => list,
                None => match name
                    .strip_prefix(verb)
                    .and_then(|rest| rest.strip_prefix('-'))
                {
                    Some(feature) => feature.to_owned(),
                    None => valued()?,
                },
            };
            for feature in list.split(',').filter(|f| !f.is_empty()) {
                let feature = feature.replace('-', "_");
                if feature != "all" && !FEATURES.contains(&feature.as_str()) {
                    out.warnings.push(format!(
                        "warning: unknown argument for --{verb}: '{feature}'"
                    ));
                    out.warnings
                        .push(format!("warning: features are [{}].", FEATURES.join(", ")));
                    continue;
                }
                out.features.push((feature, on));
            }
        }
        // The JITs, dumps and the like Spinel has nothing behind.
        _ if name.starts_with("yjit")
            || name.starts_with("zjit")
            || name.starts_with("rjit")
            || name.starts_with("jit")
            || name.starts_with("dump")
            || name == "copyright"
            || name == "parser"
            || name == "crash-report" => {}
        _ => {
            return Err(format!(
                "invalid option --{name}  (-h will show valid options; `spinel --help` for its own commands)"
            ));
        }
    }
    Ok(took_next)
}

/// `-E ext:int` and `--encoding ext:int`; a third part is refused.
fn set_encodings(value: &str, switch: &str, out: &mut RubyArgs) -> Result<(), String> {
    let mut parts = value.splitn(3, ':');
    let external = parts.next().unwrap_or("");
    let internal = parts.next();
    if let Some(extra) = parts.next() {
        return Err(format!("extra argument for {switch}: {extra}"));
    }
    if !external.is_empty() {
        out.external_encoding = Some(external.to_owned());
    }
    if let Some(internal) = internal.filter(|i| !i.is_empty()) {
        set_internal(out, internal)?;
    }
    Ok(())
}

/// Two switches naming different internal encodings is an error, measured:
/// `-Eascii:ascii -U` says the first one won.
fn set_internal(out: &mut RubyArgs, name: &str) -> Result<(), String> {
    if let Some(existing) = &out.internal_encoding
        && !existing.eq_ignore_ascii_case(name)
    {
        return Err(format!("default_internal already set to {existing}"));
    }
    out.internal_encoding = Some(name.to_owned());
    Ok(())
}

/// The Ruby that runs before the program, saying what the flags meant.
pub fn prelude(args: &RubyArgs) -> String {
    let mut out = String::new();
    let lit = |s: &str| ruby_string(s);
    match args.verbose {
        Some(Verbose::Nil) => out.push_str("$VERBOSE = nil\n"),
        Some(Verbose::False) => out.push_str("$VERBOSE = false\n"),
        Some(Verbose::True) => out.push_str("$VERBOSE = true\n"),
        None => {}
    }
    // `-w` and the levels that mean it turn the deprecation warnings on, as
    // `ruby` does; a category named with `-W:` is its own, and wins.
    if args.verbose == Some(Verbose::True) {
        out.push_str("Warning[:deprecated] = true\n");
    }
    for (category, enabled) in &args.warning_categories {
        let _ = writeln!(out, "Warning[:{category}] = {enabled} rescue nil");
    }
    if args.debug {
        out.push_str("$DEBUG = true\n");
    }
    if let Some(limit) = &args.backtrace_limit {
        let _ = writeln!(
            out,
            "Exception.__backtrace_limit__ = Integer({})",
            ruby_string(limit)
        );
    }
    // The switches that are read-only globals as well as behaviour.
    for (global, on) in [
        ("$-a", args.autosplit),
        ("$-l", args.line_ending),
        ("$-p", args.loop_mode == Some(LoopMode::Print)),
    ] {
        if on {
            let _ = writeln!(out, "__global_store__(:{global:?}, true)");
        }
    }
    if let Some(external) = &args.external_encoding {
        let _ = writeln!(out, "Encoding.default_external = {}", lit(external));
    }
    if let Some(internal) = &args.internal_encoding {
        let _ = writeln!(out, "Encoding.default_internal = {}", lit(internal));
    }
    if let Some(digits) = &args.record_separator {
        let value = u32::from_str_radix(digits, 8).unwrap_or(0);
        // Frozen, as `ruby` makes it.
        if digits.is_empty() {
            out.push_str("$/ = \"\\0\".freeze\n");
        } else if value == 0 {
            out.push_str("$/ = \"\".freeze\n");
        } else if value > 0o377 {
            out.push_str("$/ = nil\n");
        } else {
            let _ = writeln!(out, "$/ = {value}.chr.freeze");
        }
    }
    if let Some(pattern) = &args.split_pattern {
        let _ = writeln!(out, "$; = {}", lit(pattern));
    }
    if args.line_ending {
        out.push_str("$\\ = $/\n");
    }
    // `-I`'s directories, then `RUBYLIB`'s, ahead of the defaults.
    let mut dirs: Vec<String> = args
        .includes
        .iter()
        .map(|dir| format!("File.expand_path({})", lit(dir)))
        .collect();
    dirs.extend(args.library_path.iter().map(|dir| lit(dir)));
    if !dirs.is_empty() {
        let _ = writeln!(out, "$LOAD_PATH.unshift({})", dirs.join(", "));
    }
    if args.switches {
        // `-s`: leading `-name` and `-name=value` words are globals, up to a
        // `--` or the first word that is not one.
        out.push_str(
            "while (word = ARGV.first) && word.start_with?(\"-\")\n\
             \x20 ARGV.shift\n\
             \x20 break if word == \"--\"\n\
             \x20 name, value = word[1..].split(\"=\", 2)\n\
             \x20 __global_store__(:\"$#{name.tr(\"-\", \"_\")}\", value.nil? ? true : value)\n\
             end\n",
        );
    }
    if args.loop_mode.is_some() {
        // With `-n` or `-p`, and only then, `Kernel` has the four that work on
        // `$_`, private as functions and public on the module.
        out.push_str(
            "module Kernel\n\
             \x20 def chomp(*args) = $_ = $_.chomp(*args)\n\
             \x20 def chop = $_ = $_.chop\n\
             \x20 def sub(*args, &block) = $_ = $_.sub(*args, &block)\n\
             \x20 def gsub(*args, &block) = $_ = $_.gsub(*args, &block)\n\
             \x20 module_function :chomp, :chop, :sub, :gsub\n\
             end\n",
        );
    }
    for library in &args.requires {
        let _ = writeln!(out, "require {}", lit(library));
    }
    out
}

/// Wrap the program as `-n` and `-p` do: the loop opens on the program's
/// first line, so every line keeps its number.
pub fn wrap(args: &RubyArgs, source: &[u8]) -> Vec<u8> {
    let Some(mode) = args.loop_mode else {
        return source.to_vec();
    };
    let mut head = String::from("while gets;");
    if args.line_ending {
        head.push_str("$_.chomp!;");
    }
    if args.autosplit {
        head.push_str("$F = $_.split($;);");
    }
    let mut out = head.into_bytes();
    out.extend_from_slice(source);
    out.extend_from_slice(match mode {
        LoopMode::Read => b"\n;end\n".as_slice(),
        LoopMode::Print => b"\n;print $_;end\n".as_slice(),
    });
    out
}

/// The loop `-n` and `-p` put a program in, as a program of its own with
/// `__spinel_body__` where the program's statements go: what the CLI wraps
/// around a program that parses on its own, so a `BEGIN` in it stays at the
/// top level.
pub fn loop_template(args: &RubyArgs) -> Option<String> {
    let mode = args.loop_mode?;
    let mut out = String::from("while gets\n");
    if args.line_ending {
        out.push_str("$_.chomp!\n");
    }
    if args.autosplit {
        out.push_str("$F = $_.split($;)\n");
    }
    out.push_str("__spinel_body__\n");
    if mode == LoopMode::Print {
        out.push_str("print $_\n");
    }
    out.push_str("end\n");
    Some(out)
}

/// A Ruby String literal for `text`, escaping as `inspect` would.
pub fn ruby_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn flags_combine_and_take_values() {
        let parsed = parse(&args(&[
            "-wn", "-Ilib", "-r", "json", "-e", "p 1", "-e", "p 2", "a",
        ]))
        .unwrap();
        assert_eq!(parsed.verbose, Some(Verbose::True));
        assert_eq!(parsed.loop_mode, Some(LoopMode::Read));
        assert_eq!(parsed.includes, ["lib"]);
        assert_eq!(parsed.requires, ["json"]);
        assert_eq!(parsed.code.as_deref(), Some("p 1\np 2"));
        assert_eq!(parsed.argv, ["a"]);
    }

    #[test]
    fn a_script_ends_the_options() {
        let parsed = parse(&args(&["-W0", "s.rb", "-w"])).unwrap();
        assert_eq!(parsed.verbose, Some(Verbose::Nil));
        assert_eq!(parsed.script.as_deref(), Some("s.rb"));
        assert_eq!(parsed.argv, ["-w"]);
    }

    #[test]
    fn octal_record_separators() {
        assert_eq!(
            parse(&args(&["-072", "-e", "1"]))
                .unwrap()
                .record_separator
                .as_deref(),
            Some("72")
        );
        assert_eq!(
            parse(&args(&["-0", "-e", "1"]))
                .unwrap()
                .record_separator
                .as_deref(),
            Some("")
        );
    }
}

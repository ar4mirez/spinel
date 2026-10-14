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
    /// The script, or `-` for standard input.
    pub script: Option<String>,
    /// What the program sees as `ARGV`.
    pub argv: Vec<String>,
    pub requires: Vec<String>,
    pub includes: Vec<String>,
    /// `$VERBOSE`: `-W0` nil, `-W1` false, `-w`/`-W`/`-W2`/`-v` true.
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
    pub syntax_only: bool,
    /// `-v`: the version banner first, and with no program nothing else.
    pub print_version: bool,
    pub version_only: bool,
    pub external_encoding: Option<String>,
    pub internal_encoding: Option<String>,
    pub backtrace_limit: Option<String>,
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

/// Whether a command line is a `ruby` one: a flag that is not one of
/// `spinel`'s own, or `-` for a script on standard input.
pub fn is_ruby_invocation(first: &str) -> bool {
    first.starts_with('-') && !matches!(first, "-h" | "--help" | "-V")
}

/// `ruby.c`'s `proc_options`, over everything after the program name.
pub fn parse(args: &[String]) -> Result<RubyArgs, String> {
    let mut out = RubyArgs::default();
    let mut code: Vec<String> = Vec::new();
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
            parse_long(long, &mut out)?;
            i += 1;
            continue;
        }
        // Short flags combine, `-wn`; one that takes a value takes the rest
        // of the word, or else the next word where `ruby` allows it.
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut j = 0;
        while j < chars.len() {
            let c = chars[j];
            let rest: String = chars[j + 1..].iter().collect();
            match c {
                'a' => out.autosplit = true,
                'c' => out.syntax_only = true,
                'd' => out.debug = true,
                'l' => out.line_ending = true,
                'n' => {
                    if out.loop_mode.is_none() {
                        out.loop_mode = Some(LoopMode::Read);
                    }
                }
                'p' => out.loop_mode = Some(LoopMode::Print),
                's' => out.switches = true,
                'S' | 'y' => {}
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
                'e' | 'r' | 'I' | 'C' | 'E' => {
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
                        'C' => out.chdir = Some(value),
                        _ => set_encodings(&value, &mut out),
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
                'K' | 'U' | 'T' => {
                    // Source and locale encodings `ruby` once had; nothing to do.
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

fn parse_long(long: &str, out: &mut RubyArgs) -> Result<(), String> {
    let (name, value) = match long.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (long, None),
    };
    match name {
        "version" => {
            out.print_version = true;
            out.version_only = true;
        }
        "verbose" => out.verbose = Some(Verbose::True),
        "debug" => out.debug = true,
        "help" => out.help = true,
        "encoding" => set_encodings(value.unwrap_or(""), out),
        "external-encoding" => out.external_encoding = value.map(str::to_owned),
        "internal-encoding" => out.internal_encoding = value.map(str::to_owned),
        "backtrace-limit" => out.backtrace_limit = value.map(str::to_owned),
        // Features Spinel has none of, or always has: gems, did_you_mean,
        // error_highlight, rubyopt, frozen-string-literal, the JITs.
        _ if name.starts_with("disable")
            || name.starts_with("enable")
            || name.starts_with("yjit")
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
    Ok(())
}

fn set_encodings(value: &str, out: &mut RubyArgs) {
    let (external, internal) = match value.split_once(':') {
        Some((external, internal)) => (external, Some(internal)),
        None => (value, None),
    };
    if !external.is_empty() {
        out.external_encoding = Some(external.to_owned());
    }
    if let Some(internal) = internal.filter(|i| !i.is_empty()) {
        out.internal_encoding = Some(internal.to_owned());
    }
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
    for (category, enabled) in &args.warning_categories {
        let _ = writeln!(out, "Warning[:{category}] = {enabled} rescue nil");
    }
    if args.debug {
        out.push_str("$DEBUG = true\n");
    }
    if let Some(external) = &args.external_encoding {
        let _ = writeln!(out, "Encoding.default_external = {}", lit(external));
    }
    if let Some(internal) = &args.internal_encoding {
        let _ = writeln!(out, "Encoding.default_internal = {}", lit(internal));
    }
    if let Some(digits) = &args.record_separator {
        let value = u32::from_str_radix(digits, 8).unwrap_or(0);
        if digits.is_empty() {
            out.push_str("$/ = \"\\0\"\n");
        } else if value == 0 {
            out.push_str("$/ = \"\"\n");
        } else if value > 0o377 {
            out.push_str("$/ = nil\n");
        } else {
            let _ = writeln!(out, "$/ = {value}.chr");
        }
    }
    if let Some(pattern) = &args.split_pattern {
        let _ = writeln!(out, "$; = {}", lit(pattern));
    }
    if args.line_ending {
        out.push_str("$\\ = $/\n");
    }
    for dir in args.includes.iter().rev() {
        let _ = writeln!(out, "$LOAD_PATH.unshift(File.expand_path({}))", lit(dir));
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

/// A Ruby String literal for `text`, escaping as `inspect` would.
fn ruby_string(text: &str) -> String {
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

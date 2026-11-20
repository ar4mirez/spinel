//! Replay `oracle.txt` — measured from CRuby — through this engine.
//!
//! Every line is a pattern ruby/spec's `language/regexp/` actually uses and the
//! answer a real Ruby gives for it on each of thirty probe subjects. A
//! disagreement here is a wrong answer, which is the one outcome
//! `docs/engine.md` will not accept from a regex backend.
//!
//! A pattern this engine *refuses* is not a disagreement: it is counted,
//! reported, and held below a ceiling, because a refusal reaches a spec run
//! as "blocked" rather than as a pass. Silence is what is forbidden,
//! not incompleteness.

use spinel_regex::{Encoding, Error, Flags, Regex};

const TABLE: &str = include_str!("oracle.txt");

/// The probe subjects, in the order `scripts/regexp-oracle.rb` writes them.
const INPUTS: &[&str] = &[
    "", "foo", "foo\n", "\nfoo", "foo\nbar", "FOO", "a", "ab", "abc", "aaa", "foo bar", " foo ",
    "x\ny", "1", "12", "a1b2", "\t", "é", "あ", "٣", "()", "[c]", "aXb", " ", "a b", "foobar",
    "barfoo", "\r\n", "-", "_",
];

/// Ruby's `String#inspect` output, back to the string it stands for.
fn unquote(quoted: &str) -> String {
    let inner = &quoted[1..quoted.len() - 1];
    let mut out = String::new();
    let mut it = inner.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('e') => out.push('\x1b'),
            Some('s') => out.push(' '),
            Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'),
            Some('f') => out.push('\x0c'),
            Some('v') => out.push('\x0b'),
            Some('0') => out.push('\0'),
            Some('u') => {
                let mut hex = String::new();
                if it.clone().next() == Some('{') {
                    it.next();
                    for c in it.by_ref() {
                        if c == '}' {
                            break;
                        }
                        hex.push(c);
                    }
                } else {
                    for _ in 0..4 {
                        if let Some(c) = it.next() {
                            hex.push(c);
                        }
                    }
                }
                if let Some(c) = u32::from_str_radix(hex.trim(), 16)
                    .ok()
                    .and_then(char::from_u32)
                {
                    out.push(c);
                }
            }
            Some('x') => {
                let mut hex = String::new();
                while hex.len() < 2 {
                    match it.clone().next() {
                        Some(c) if c.is_ascii_hexdigit() => {
                            hex.push(c);
                            it.next();
                        }
                        _ => break,
                    }
                }
                if let Ok(n) = u32::from_str_radix(&hex, 16) {
                    // A lone byte above 0x7f is not a character; the oracle
                    // marks those patterns rejected anyway.
                    out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                }
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// Ruby reports match offsets in characters; this engine reports bytes.
fn to_chars(haystack: &str, byte: usize) -> usize {
    haystack[..byte].chars().count()
}

/// One probe subject's answer, in the table's own notation.
fn answer(re: &Regex, subject: &str) -> String {
    match re.find_at(subject, 0) {
        Err(_) => "!".to_owned(),
        Ok(None) => "-".to_owned(),
        Ok(Some(caps)) => {
            let mut fields = Vec::new();
            for group in 0..=re.group_count() {
                match caps.group(group) {
                    Some((s, e)) => {
                        fields.push(format!("{},{}", to_chars(subject, s), to_chars(subject, e)));
                    }
                    None => fields.push("~".to_owned()),
                }
            }
            fields.join(":")
        }
    }
}

/// Patterns this engine is known to answer differently from CRuby, each with
/// the reason. Nothing is added here to make a run green: a divergence listed
/// here is printed on every run, counted, and meant to be deleted.
///
/// Empty since #177. It held `^(()|a|())*?$`: Onigmo keeps a capture that an
/// iteration set even after backtracking out of that iteration, while a capture
/// set outside a loop is rolled back, and this engine snapshotted at every
/// backtrack point so it reset in both places. The machine now commits a
/// completed iteration's captures instead, and `capture_restore_records` below
/// is the pair of cases that pins the rule in both directions.
const KNOWN_DIVERGENCES: &[&str] = &[];

struct Tally {
    agreed: usize,
    refused: Vec<(String, &'static str)>,
    disagreed: Vec<String>,
}

fn replay() -> Tally {
    replay_table(TABLE, INPUTS)
}

fn replay_table(table: &str, inputs: &[&str]) -> Tally {
    let mut tally = Tally {
        agreed: 0,
        refused: Vec::new(),
        disagreed: Vec::new(),
    };

    for line in table.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let Some((quoted, expected)) = line.split_once('\t') else {
            continue;
        };
        let source = unquote(quoted);

        // Ruby rejected the pattern: this engine must reject it too, for any
        // reason. Which message it gives is not the oracle's business.
        if let Some(stripped) = expected.strip_prefix('!') {
            let _ = stripped;
            match Regex::new(&source, Flags::default()) {
                Err(_) => tally.agreed += 1,
                Ok(_) => tally.disagreed.push(format!(
                    "{quoted}: Ruby rejects this pattern, the engine accepts it"
                )),
            }
            continue;
        }

        // A pattern from a `# encoding: binary` spec file (`escapes_spec.rb`'s
        // `/\xAA/`) is a binary Regexp in Ruby, which raises rather than
        // answers for a subject past ASCII. It is compiled as one here, and
        // only the subjects Ruby answered for are compared: the raise is the
        // VM's, not the engine's.
        const INCOMPATIBLE: &str = "!Encoding::CompatibilityError";
        let binary = expected.split(' ').any(|answer| answer == INCOMPATIBLE);
        let compiled = if binary {
            Regex::with_encoding(&source, Flags::default(), Encoding::Binary)
        } else {
            Regex::new(&source, Flags::default())
        };
        let re = match compiled {
            Ok(re) => re,
            // A refusal is a documented gap, not a wrong answer.
            Err(Error::Unsupported(what)) => {
                tally.refused.push((quoted.to_owned(), what));
                continue;
            }
            Err(e) => {
                tally.disagreed.push(format!(
                    "{quoted}: Ruby accepts this pattern, the engine rejects it: {e}"
                ));
                continue;
            }
        };

        let got: Vec<String> = inputs.iter().map(|s| answer(&re, s)).collect();
        let want: Vec<&str> = expected.split(' ').collect();
        if got.len() != want.len() {
            tally.disagreed.push(format!("{quoted}: field count"));
            continue;
        }
        let mut mismatched = Vec::new();
        for ((subject, want), got) in inputs.iter().zip(&want).zip(&got) {
            if want != got && !(binary && *want == INCOMPATIBLE) {
                mismatched.push(format!("    on {subject:?}: ruby {want}, engine {got}"));
            }
        }
        if mismatched.is_empty() {
            tally.agreed += 1;
        } else {
            tally
                .disagreed
                .push(format!("{quoted}\n{}", mismatched.join("\n")));
        }
    }
    tally
}

/// The engine must never disagree with CRuby about a pattern it accepted,
/// except for the divergences named and explained above.
#[test]
fn agrees_with_cruby_on_every_pattern_it_accepts() {
    let tally = replay();
    let unexplained: Vec<&String> = tally
        .disagreed
        .iter()
        .filter(|report| {
            let pattern = report.lines().next().unwrap_or("");
            !KNOWN_DIVERGENCES
                .iter()
                .any(|known| pattern.starts_with(known))
        })
        .collect();

    for known in KNOWN_DIVERGENCES {
        println!("known divergence, still open: {known}");
    }
    assert!(
        unexplained.is_empty(),
        "{} of {} patterns disagree with CRuby for reasons nobody has written down:\n\n{}",
        unexplained.len(),
        tally.agreed + tally.disagreed.len(),
        unexplained
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    assert_eq!(
        tally.disagreed.len(),
        KNOWN_DIVERGENCES.len(),
        "a known divergence was fixed: delete it from KNOWN_DIVERGENCES"
    );
}

/// Refusals are allowed but bounded: a gap that grows silently is a gap nobody
/// is closing. Lower this number as constructs land; never raise it.
#[test]
fn refusals_stay_within_their_budget() {
    const BUDGET: usize = 2;
    let tally = replay();
    let mut kinds: Vec<&str> = tally.refused.iter().map(|(_, what)| *what).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert!(
        tally.refused.len() <= BUDGET,
        "{} patterns refused, budget is {BUDGET}. Refused for: {kinds:?}",
        tally.refused.len(),
    );
    println!(
        "agreed {}, refused {} ({kinds:?})",
        tally.agreed,
        tally.refused.len()
    );
}

/// The double-quoted strings in `text`, each with its quotes.
fn quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut quoted = String::from('"');
        while let Some(c) = chars.next() {
            quoted.push(c);
            match c {
                '\\' => quoted.extend(chars.next()),
                '"' => break,
                _ => {}
            }
        }
        out.push(quoted);
    }
    out
}

/// The folds past one character to one (#788), measured against subjects of
/// their own: `ß` against `ss` both ways round, in strings Onigmo expands and
/// ones it compares folded, in classes, under repetition, in backreferences
/// and look-behind. No divergence is allowed here and no pattern refused.
#[test]
fn agrees_with_cruby_on_multi_character_folds() {
    const FOLDS: &str = include_str!("folds.txt");
    // The subjects are the header's, quoted as Ruby inspects them, rather
    // than a second copy here: several are lookalikes, and a copy that drifts
    // compares the wrong strings.
    let quoted: Vec<String> = FOLDS
        .lines()
        .skip_while(|line| !line.ends_with("in order:"))
        .skip(1)
        .take_while(|line| line.starts_with("#   "))
        .flat_map(|line| quoted_strings(&line[4..]))
        .collect();
    let subjects: Vec<String> = quoted.iter().map(|q| unquote(q)).collect();
    let inputs: Vec<&str> = subjects.iter().map(String::as_str).collect();
    let tally = replay_table(FOLDS, &inputs);
    assert!(tally.refused.is_empty(), "refused: {:?}", tally.refused);
    assert!(
        tally.disagreed.is_empty(),
        "{} of {} patterns disagree with CRuby:\n\n{}",
        tally.disagreed.len(),
        tally.agreed + tally.disagreed.len(),
        tally.disagreed.join("\n\n")
    );
}

/// The two halves of Onigmo's capture-restore rule, which are only interesting
/// together: an engine that rolls back everywhere fails the first pair, and one
/// that rolls back nowhere fails the second.
///
/// Measured rather than reasoned — all four patterns are in `oracle.txt` too,
/// against the full probe corpus. This test exists so the rule has a name in the
/// file, and so a change that trades one case for the other says which.
#[test]
fn capture_restore_records() {
    let caps = |pattern: &str, subject: &'static str| {
        let re = Regex::new(pattern, Flags::default()).expect("compiles");
        let m = re.find_at(subject, 0).expect("runs").expect("matches");
        (1..=re.group_count())
            .map(|g| m.group(g).map(|(s, e)| &subject[s..e]))
            .collect::<Vec<_>>()
    };

    // Survives: the iteration that set `$2` completed, so abandoning it later in
    // favour of matching `a` leaves `""` behind rather than nil.
    assert_eq!(caps("^(()|a)*?$", "aa"), vec![Some("a"), Some("")]);
    assert_eq!(
        caps("^(()|a|())*?$", "aaa"),
        vec![Some("a"), Some(""), None]
    );

    // Rolled back: `(a)` matched before `x` failed, and no iteration completed
    // in between, so `$1` is nil once the alternation moves on to `ab`.
    assert_eq!(caps("(?:(a)x|ab)", "ab"), vec![None]);
    // The same rollback inside a loop, for the same reason: `(a)x` failed part
    // way through the iteration, which therefore never committed.
    assert_eq!(caps("((a)x|a)*", "aa"), vec![Some("a"), None]);
}

//! Replay `oracle.txt` — measured from CRuby — through this engine.
//!
//! Every line is a pattern ruby/spec's `language/regexp/` actually uses and the
//! answer a real Ruby gives for it on each of thirty probe subjects. A
//! disagreement here is a wrong answer, which is the one outcome
//! `docs/engine.md` will not accept from a regex backend.
//!
//! A pattern this engine *refuses* is not a disagreement: it is counted,
//! reported, and held below a ceiling, because a refusal reaches the spec
//! harness as "blocked" rather than as a pass. Silence is what is forbidden,
//! not incompleteness.

use spinel_regex::{Error, Flags, Regex};

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
/// `^(()|a|())*?$` — Onigmo keeps a capture that an iteration set even after
/// backtracking out of that iteration, but only inside a loop: at the top level
/// `(?:(a)x|ab)` resets group 1, measured. This engine snapshots captures at
/// every backtrack point, so it resets in both places. Closing it means
/// modelling Onigmo's capture-restore records rather than whole snapshots,
/// which is its own slice. Reachable only from a lazy repeat over empty
/// alternations, which is `empty_checks_spec.rb` and not real Ruby code.
const KNOWN_DIVERGENCES: &[&str] = &["\"^(()|a|())*?$\""];

struct Tally {
    agreed: usize,
    refused: Vec<(String, &'static str)>,
    disagreed: Vec<String>,
}

fn replay() -> Tally {
    let mut tally = Tally {
        agreed: 0,
        refused: Vec::new(),
        disagreed: Vec::new(),
    };

    for line in TABLE.lines() {
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

        let re = match Regex::new(&source, Flags::default()) {
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

        let got: Vec<String> = INPUTS.iter().map(|s| answer(&re, s)).collect();
        let want: Vec<&str> = expected.split(' ').collect();
        if got.len() != want.len() {
            tally.disagreed.push(format!("{quoted}: field count"));
            continue;
        }
        let mut mismatched = Vec::new();
        for ((subject, want), got) in INPUTS.iter().zip(&want).zip(&got) {
            if want != got {
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
    const BUDGET: usize = 40;
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

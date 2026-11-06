//! `\X`: one extended grapheme cluster, by the rules of Unicode's UAX #29,
//! over the `Grapheme_Cluster_Break`, `InCB` and `Extended_Pictographic`
//! sets `tests/properties.txt` measured from CRuby.
//!
//! The rules are written out here and the sets are measured, so the Unicode
//! version is CRuby's for every character and the rules are the ones its
//! Onigmo builds `\X` from. `scripts/regexp-oracle.rb` checks the two
//! together only on its probe subjects, through the `\X` rows of
//! `tests/oracle.txt`; the cluster tests in `lib.rs` hold emoji sequences,
//! Hangul, regional indicators and GB9c, each as CRuby splits it.

use crate::property::{self, Property};
use crate::{Encoding, exec::Subject};

/// A character's `Grapheme_Cluster_Break` value, with `Other` for the rest.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Break {
    Cr,
    Lf,
    Control,
    Extend,
    Zwj,
    RegionalIndicator,
    Prepend,
    SpacingMark,
    L,
    V,
    T,
    Lv,
    Lvt,
    Other,
}

/// A character's `InCB` value, which only GB9c reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Incb {
    Consonant,
    Extend,
    Linker,
    None,
}

/// Everything the rules ask of one character.
#[derive(Clone, Copy)]
struct Char {
    gcb: Break,
    incb: Incb,
    pictographic: bool,
}

fn has(name: &str, c: char) -> bool {
    property::named(name).is_some_and(|p: Property| property::contains(p, c))
}

fn classify(c: char) -> Char {
    if c.is_ascii() {
        let gcb = match c {
            '\r' => Break::Cr,
            '\n' => Break::Lf,
            c if c.is_ascii_control() => Break::Control,
            _ => Break::Other,
        };
        return Char {
            gcb,
            incb: Incb::None,
            pictographic: false,
        };
    }
    const BREAKS: &[(&str, Break)] = &[
        ("graphemeclusterbreak=extend", Break::Extend),
        ("graphemeclusterbreak=zwj", Break::Zwj),
        ("graphemeclusterbreak=spacingmark", Break::SpacingMark),
        ("graphemeclusterbreak=control", Break::Control),
        (
            "graphemeclusterbreak=regionalindicator",
            Break::RegionalIndicator,
        ),
        ("graphemeclusterbreak=prepend", Break::Prepend),
        ("graphemeclusterbreak=l", Break::L),
        ("graphemeclusterbreak=v", Break::V),
        ("graphemeclusterbreak=t", Break::T),
        ("graphemeclusterbreak=lv", Break::Lv),
        ("graphemeclusterbreak=lvt", Break::Lvt),
    ];
    let gcb = BREAKS
        .iter()
        .find(|(name, _)| has(name, c))
        .map_or(Break::Other, |&(_, gcb)| gcb);
    let incb = if has("incb=linker", c) {
        Incb::Linker
    } else if has("incb=consonant", c) {
        Incb::Consonant
    } else if has("incb=extend", c) {
        Incb::Extend
    } else {
        Incb::None
    };
    Char {
        gcb,
        incb,
        pictographic: has("extendedpictographic", c),
    }
}

/// Where the cluster that starts at `sp` ends, or `None` at the end of the
/// subject. Outside UTF-8 a cluster is a CRLF pair or one character, which is
/// Onigmo's `\X` for an encoding without Unicode's tables.
pub(crate) fn grapheme_end(haystack: Subject<'_>, sp: usize) -> Option<usize> {
    let (first, width) = haystack.next_char(sp)?;
    let mut end = sp + width;
    if haystack.enc != Encoding::Utf8 {
        if first == '\r' && haystack.bytes.get(end) == Some(&b'\n') {
            end += 1;
        }
        return Some(end);
    }
    let mut prev = classify(first);
    // GB12 and GB13: how many regional indicators run back from here.
    let mut indicators = usize::from(prev.gcb == Break::RegionalIndicator);
    // GB11: an `Extended_Pictographic` then `Extend*`, and whether a ZWJ
    // has closed it.
    let mut pictograph = prev.pictographic;
    let mut joined = false;
    // GB9c: an `InCB=Consonant`, then `InCB=Extend` or `InCB=Linker`, and
    // whether a linker has been among them.
    let mut consonant = prev.incb == Incb::Consonant;
    let mut linked = false;
    while let Some((c, width)) = haystack.next_char(end) {
        let next = classify(c);
        if !joins(prev, next, indicators, joined, linked) {
            break;
        }
        end += width;
        indicators = if next.gcb == Break::RegionalIndicator {
            indicators + 1
        } else {
            0
        };
        (pictograph, joined) = match next.gcb {
            _ if next.pictographic => (true, false),
            Break::Extend if pictograph && !joined => (true, false),
            Break::Zwj if pictograph && !joined => (true, true),
            _ => (false, false),
        };
        (consonant, linked) = match next.incb {
            Incb::Consonant => (true, false),
            Incb::Linker if consonant => (true, true),
            Incb::Extend if consonant => (true, linked),
            _ => (false, false),
        };
        prev = next;
    }
    Some(end)
}

/// Whether no boundary falls between `prev` and `next`, given the state the
/// rules that look further back need. UAX #29's rules in their own order.
fn joins(prev: Char, next: Char, indicators: usize, joined: bool, linked: bool) -> bool {
    use Break::*;
    match (prev.gcb, next.gcb) {
        // GB3
        (Cr, Lf) => true,
        // GB4, GB5
        (Cr | Lf | Control, _) | (_, Cr | Lf | Control) => false,
        // GB6, GB7, GB8
        (L, L | V | Lv | Lvt) | (Lv | V, V | T) | (Lvt | T, T) => true,
        // GB9, GB9a, GB9b
        (_, Extend | Zwj | SpacingMark) | (Prepend, _) => true,
        // GB9c
        _ if linked && next.incb == Incb::Consonant => true,
        // GB11
        (Zwj, _) if joined && next.pictographic => true,
        // GB12, GB13
        (RegionalIndicator, RegionalIndicator) => indicators % 2 == 1,
        // GB999
        _ => false,
    }
}

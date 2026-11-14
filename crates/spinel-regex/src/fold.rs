//! Unicode's full case folding, and how Onigmo matches a string by it (#788).
//!
//! Under `/i` a UTF-8 pattern's `ß` matches `"ss"` and its `ss` matches
//! `"ß"`: a fold can turn one character into two or three. Onigmo does not
//! get there by folding both sides and comparing, and its answers show the
//! difference — `/(?i)s(?:s)/` misses `"ß"` while `/(?i)ss/` finds it, and
//! `/(?i)sß/` misses `"ßs"` while `/(?i)sss/` finds it. What it does instead,
//! in `regcomp.c`'s `expand_case_fold_string`, is rewrite each string the
//! pattern spells into alternatives: at each character, the character itself
//! and every way of spelling it in another case, including the longer and
//! shorter spellings, until the count of alternatives passes eight; the rest
//! of the string is then compared folded, a subject character at a time. This
//! module is that rewrite, over the tables `build.rs` makes from the measured
//! `tests/casefold.txt`.
//!
//! Only a UTF-8 subject folds this way. Every other encoding keeps the one
//! character to one character folds `exec.rs` compares by.
//!
//! ponytail: ISO-8859-1 and the other single-byte encodings with a `ß` fold it
//! to `ss` in Onigmo too (`/(?i)\xDF/n`-style patterns in those encodings);
//! here they fold one character to one. Their tables would need the same
//! measurement `single_byte.rs` has for the one-to-one folds.

include!(concat!(env!("OUT_DIR"), "/casefold_tables.rs"));

/// Onigmo's `THRESHOLD_CASE_FOLD_ALT_FOR_EXPANSION`: once the product of the
/// alternatives at each character passes this, the rest of the string is
/// compared folded instead of expanded.
const THRESHOLD: usize = 8;

/// A character's full case fold: one to three characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Folded {
    chars: [char; 3],
    len: u8,
}

impl Folded {
    fn one(c: char) -> Folded {
        Folded {
            chars: [c, '\0', '\0'],
            len: 1,
        }
    }

    pub(crate) fn as_slice(&self) -> &[char] {
        &self.chars[..usize::from(self.len)]
    }
}

/// The table's entry for `c`, if it folds to anything but itself.
fn lookup(c: char) -> Option<&'static [u32; 3]> {
    FOLD.binary_search_by_key(&u32::from(c), |&(code, _)| code)
        .ok()
        .map(|i| &FOLD[i].1)
}

fn to_char(code: u32) -> char {
    char::from_u32(code).expect("casefold.txt holds scalar values")
}

/// `c`'s full case fold.
pub(crate) fn fold(c: char) -> Folded {
    if c.is_ascii() {
        return Folded::one(c.to_ascii_lowercase());
    }
    match lookup(c) {
        None => Folded::one(c),
        Some(codes) => {
            let mut folded = Folded::one(to_char(codes[0]));
            for &code in codes[1..].iter().take_while(|&&code| code != 0) {
                folded.chars[usize::from(folded.len)] = to_char(code);
                folded.len += 1;
            }
            folded
        }
    }
}

/// `c`'s fold when it is one character, else `c`: what Onigmo keys its
/// two- and three-character unfold tables by.
fn fold_one(c: char) -> char {
    match fold(c) {
        f if f.len == 1 => f.chars[0],
        _ => c,
    }
}

fn unfold1(target: char) -> &'static [u32] {
    UNFOLD1
        .binary_search_by_key(&u32::from(target), |&(code, _)| code)
        .map_or(&[], |i| UNFOLD1[i].1)
}

fn unfold2(a: char, b: char) -> &'static [u32] {
    let key = [u32::from(a), u32::from(b)];
    UNFOLD2
        .binary_search_by_key(&key, |&(codes, _)| codes)
        .map_or(&[], |i| UNFOLD2[i].1)
}

fn unfold3(a: char, b: char, c: char) -> &'static [u32] {
    let key = [u32::from(a), u32::from(b), u32::from(c)];
    UNFOLD3
        .binary_search_by_key(&key, |&(codes, _)| codes)
        .map_or(&[], |i| UNFOLD3[i].1)
}

/// The characters that fold to `folded`: the characters `c` matches under
/// `/i`, with `c` itself among them.
pub(crate) fn same_fold(folded: Folded) -> impl Iterator<Item = char> {
    let chars = folded.as_slice();
    let (target, sources) = match chars {
        [a] => (Some(*a), unfold1(*a)),
        [a, b] => (None, unfold2(*a, *b)),
        [a, b, c] => (None, unfold3(*a, *b, *c)),
        _ => (None, &[][..]),
    };
    target
        .into_iter()
        .chain(sources.iter().map(|&code| to_char(code)))
}

/// One way the subject may spell the pattern's text at a position: `codes`
/// in the subject for `len` characters of the pattern.
#[derive(Debug)]
struct Item {
    len: usize,
    codes: Vec<char>,
}

/// Onigmo's `onigenc_unicode_get_case_fold_codes_by_str`: every other
/// spelling of the text at the start of `p`, in its order.
fn items(p: &[char]) -> Vec<Item> {
    let one = |c: char| Item {
        len: 1,
        codes: vec![c],
    };
    let mut out = Vec::new();
    let c0 = p[0];
    let mut code = c0;
    let mut longer = true;
    let entry = if c0.is_ascii() {
        // ASCII's folds are in the table too; this is the same answer
        // without the search.
        c0.is_ascii_uppercase()
            .then(|| [u32::from(c0.to_ascii_lowercase()), 0, 0])
    } else {
        lookup(c0).copied()
    };
    match entry {
        Some([target, 0, 0]) => {
            let target = to_char(target);
            out.push(one(target));
            out.extend(
                unfold1(target)
                    .iter()
                    .map(|&c| to_char(c))
                    .filter(|&c| c != c0)
                    .map(one),
            );
            code = target;
        }
        Some(codes) => {
            let cases: Vec<Vec<char>> = codes
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| {
                    std::iter::once(to_char(c))
                        .chain(unfold1(to_char(c)).iter().map(|&c| to_char(c)))
                        .collect()
                })
                .collect();
            let whole: &[u32] = if cases.len() == 2 {
                for &a in &cases[0] {
                    for &b in &cases[1] {
                        out.push(Item {
                            len: 1,
                            codes: vec![a, b],
                        });
                    }
                }
                unfold2(to_char(codes[0]), to_char(codes[1]))
            } else {
                for &a in &cases[0] {
                    for &b in &cases[1] {
                        for &c in &cases[2] {
                            out.push(Item {
                                len: 1,
                                codes: vec![a, b, c],
                            });
                        }
                    }
                }
                unfold3(to_char(codes[0]), to_char(codes[1]), to_char(codes[2]))
            };
            out.extend(
                whole
                    .iter()
                    .map(|&c| to_char(c))
                    .filter(|&c| c != c0)
                    .map(one),
            );
            // A character that folds to several is never the head of a
            // longer sequence that folds to one.
            longer = false;
        }
        None => out.extend(unfold1(c0).iter().map(|&c| to_char(c)).map(one)),
    }
    if longer && p.len() > 1 {
        let c1 = fold_one(p[1]);
        out.extend(unfold2(code, c1).iter().map(|&c| Item {
            len: 2,
            codes: vec![to_char(c)],
        }));
        if p.len() > 2 {
            let c2 = fold_one(p[2]);
            out.extend(unfold3(code, c1, c2).iter().map(|&c| Item {
                len: 3,
                codes: vec![to_char(c)],
            }));
        }
    }
    out
}

/// What a string the pattern spells becomes under `/i` in UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Piece {
    /// These characters exactly, with no case folding: one spelling.
    Exact(Vec<char>),
    /// The subject, folded a character at a time, must spell these folded
    /// characters exactly, ending on a character's end: Onigmo's
    /// `EXACT*_IC`. `/(?i)ß/` is this, `ss`, which is why it matches `"ss"`,
    /// `"ß"`, `"ẞ"` and `"ſS"` alike.
    Folded(Vec<char>),
    List(Vec<Piece>),
    Alt(Vec<Piece>),
}

fn folded(text: &[char]) -> Piece {
    Piece::Folded(
        text.iter()
            .flat_map(|&c| fold(c).as_slice().to_vec())
            .collect(),
    )
}

/// `text`, a string the pattern spells under `/i`, as Onigmo rewrites it.
/// In a look-behind nothing is expanded and the whole string is compared
/// folded.
pub(crate) fn expand(text: &[char], look_behind: bool) -> Piece {
    let mut alternatives = 1;
    let mut pieces = expand_from(text, 0, look_behind, &mut alternatives);
    if pieces.len() == 1 {
        pieces.pop().expect("just checked there is one")
    } else {
        Piece::List(pieces)
    }
}

/// The pieces of `text` from `at` on. A character with a spelling of another
/// length opens an alternation whose first branch holds everything after it,
/// which is where Onigmo appends the rest of the string.
fn expand_from(
    text: &[char],
    mut at: usize,
    look_behind: bool,
    alternatives: &mut usize,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut plain = at;
    while at < text.len() {
        let items = items(&text[at..]);
        let varies = items
            .iter()
            .any(|item| item.len != 1 || item.codes.len() != 1);
        if items.is_empty() || !varies || look_behind {
            at += 1;
            continue;
        }
        *alternatives *= items.len() + 1;
        if *alternatives > THRESHOLD {
            break;
        }
        if plain < at {
            pieces.push(folded(&text[plain..at]));
        }
        let mut same = vec![Piece::Exact(vec![text[at]])];
        let mut longer = Vec::new();
        for item in items {
            if item.len == 1 {
                same.push(Piece::Exact(item.codes));
            } else if at + item.len < text.len() {
                longer.push(Piece::List(vec![
                    Piece::Exact(item.codes),
                    folded(&text[at + item.len..]),
                ]));
            } else {
                longer.push(Piece::Exact(item.codes));
            }
        }
        at += 1;
        if longer.is_empty() {
            pieces.push(Piece::Alt(same));
            plain = at;
            continue;
        }
        let mut first = vec![Piece::Alt(same)];
        first.extend(expand_from(text, at, look_behind, alternatives));
        let mut branches = vec![Piece::List(first)];
        branches.extend(longer);
        pieces.push(Piece::Alt(branches));
        return pieces;
    }
    if plain < at {
        pieces.push(folded(&text[plain..at]));
    }
    if at < text.len() {
        pieces.push(folded(&text[at..]));
    }
    pieces
}

/// Whether `text`, compared folded, can only ever match one subject
/// character per character of its own — every character folds to one, and
/// no two or three in a row fold to what one character does — so that
/// matching it a character at a time gives the same answer.
pub(crate) fn one_to_one(text: &[char]) -> bool {
    text.iter().all(|&c| fold(c).len == 1)
        && text
            .windows(2)
            .all(|w| unfold2(fold_one(w[0]), fold_one(w[1])).is_empty())
        && text
            .windows(3)
            .all(|w| unfold3(fold_one(w[0]), fold_one(w[1]), fold_one(w[2])).is_empty())
}

/// Where the subject, folded a character at a time from `at`, finishes
/// spelling `want`, if it does so exactly.
pub(crate) fn folded_end(
    mut next: impl FnMut(usize) -> Option<(char, usize)>,
    mut at: usize,
    want: &[char],
) -> Option<usize> {
    let mut matched = 0;
    while matched < want.len() {
        let (c, width) = next(at)?;
        for &f in fold(c).as_slice() {
            if want.get(matched) != Some(&f) {
                return None;
            }
            matched += 1;
        }
        at += width;
    }
    Some(at)
}

/// Every character that folds to two or three, in the order Onigmo's
/// `onigenc_unicode_apply_all_case_fold` visits them: the two-character
/// folds before the three, each by its folded text.
pub(crate) fn multi_folds() -> impl Iterator<Item = (Folded, char)> {
    let two = UNFOLD2.iter().flat_map(|(key, from)| {
        let folded = Folded {
            chars: [to_char(key[0]), to_char(key[1]), '\0'],
            len: 2,
        };
        from.iter().map(move |&c| (folded, to_char(c)))
    });
    let three = UNFOLD3.iter().flat_map(|(key, from)| {
        let folded = Folded {
            chars: [to_char(key[0]), to_char(key[1]), to_char(key[2])],
            len: 3,
        };
        from.iter().map(move |&c| (folded, to_char(c)))
    });
    two.chain(three)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_are_unicode_full_folds() {
        assert_eq!(fold('ß').as_slice(), &['s', 's']);
        assert_eq!(fold('ẞ').as_slice(), &['s', 's']);
        assert_eq!(fold('ﬃ').as_slice(), &['f', 'f', 'i']);
        assert_eq!(fold('ſ').as_slice(), &['s']);
        assert_eq!(fold('K').as_slice(), &['k']);
        assert_eq!(fold('\u{212A}').as_slice(), &['k']);
        assert_eq!(fold('İ').as_slice(), &['i', '\u{307}']);
        assert_eq!(fold('é').as_slice(), &['é']);
    }

    #[test]
    fn same_fold_is_every_spelling() {
        let mut s: Vec<char> = same_fold(fold('s')).collect();
        s.sort_unstable();
        assert_eq!(s, vec!['S', 's', 'ſ']);
        let mut ss: Vec<char> = same_fold(fold('ß')).collect();
        ss.sort_unstable();
        assert_eq!(ss, vec!['ß', 'ẞ']);
    }

    #[test]
    fn a_string_expands_as_onigmo_rewrites_it() {
        // `ß` alone has ten other spellings, past the threshold: compared
        // folded, all of it.
        assert_eq!(expand(&['ß'], false), Piece::Folded(vec!['s', 's']));
        // `ss`: the first `s` may be `ß` or `ẞ` with the second, and then the
        // count passes eight, so the rest is folded.
        assert_eq!(
            expand(&['s', 's'], false),
            Piece::Alt(vec![
                Piece::List(vec![
                    Piece::Alt(vec![
                        Piece::Exact(vec!['s']),
                        Piece::Exact(vec!['S']),
                        Piece::Exact(vec!['ſ']),
                    ]),
                    Piece::Folded(vec!['s']),
                ]),
                Piece::Exact(vec!['ß']),
                Piece::Exact(vec!['ẞ']),
            ])
        );
        // Nothing past one-to-one: a folded string.
        assert_eq!(expand(&['a', 'b'], false), Piece::Folded(vec!['a', 'b']));
        // A look-behind expands nothing.
        assert_eq!(expand(&['s', 's'], true), Piece::Folded(vec!['s', 's']));
    }

    #[test]
    fn one_to_one_spots_a_fold_across_characters() {
        assert!(one_to_one(&['a', 'b', 'c']));
        assert!(!one_to_one(&['s', 's']));
        assert!(!one_to_one(&['f', 'f', 'i']));
        assert!(!one_to_one(&['ß']));
        assert!(one_to_one(&['s']));
    }
}

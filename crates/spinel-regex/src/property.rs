//! `\p{}`: the Unicode properties a UTF-8 pattern can name, as measured from
//! CRuby into `tests/properties.txt` and turned into tables by `build.rs`.
//!
//! Measured for the reason the POSIX brackets are (#180): a Unicode-tables
//! crate would have to keep in step with the Unicode version CRuby was built
//! against, and a mismatch is a quiet per-codepoint disagreement. Here
//! `scripts/regexp-oracle.rb --check` fails CI instead.

include!(concat!(env!("OUT_DIR"), "/property_tables.rs"));

/// A property a name resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    /// One of the measured sets, by its index in `SETS`.
    Set(u16),
    /// `Age=V`: every character assigned by version `V`, which is every
    /// `AGES` entry up to and including this index.
    Age(u8),
}

/// The property a `\p{}` name reads in a UTF-8 pattern, as Onigmo looks it
/// up: spaces, hyphens and underscores dropped and the rest lowercased, so
/// `Old_Italic`, `old italic` and `OLDITALIC` are one name. A name past ASCII
/// is no property at all.
pub(crate) fn lookup(name: &str) -> Option<Property> {
    if !name.is_ascii() {
        return None;
    }
    let key: String = name
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if let Some(at) = AGES.iter().position(|(age, _)| *age == key) {
        return u8::try_from(at).ok().map(Property::Age);
    }
    named(&key)
}

/// A measured set by the name `tests/properties.txt` spells it with, already
/// lowercase and with nothing dropped: what `\X` reads its sets by, without
/// [`lookup`]'s allocation per character.
pub(crate) fn named(key: &str) -> Option<Property> {
    NAMES
        .binary_search_by(|(name, _)| (*name).cmp(key))
        .ok()
        .map(|at| Property::Set(NAMES[at].1))
}

/// Whether `c` has the property.
pub(crate) fn contains(property: Property, c: char) -> bool {
    match property {
        Property::Set(set) => crate::exec::in_ranges(SETS[usize::from(set)], c),
        Property::Age(age) => AGES[..=usize::from(age)]
            .iter()
            .any(|(_, added)| crate::exec::in_ranges(added, c)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_looked_up_as_onigmo_spells_them() {
        let letter = lookup("L");
        assert!(letter.is_some());
        assert_eq!(lookup("Letter"), letter);
        assert_eq!(lookup("l"), letter);
        assert_eq!(lookup("Old_Italic"), lookup("old italic"));
        assert!(lookup("Old-Italic").is_some());
        assert_eq!(lookup("In_Basic_Latin"), lookup("InBasicLatin"));
        assert_eq!(lookup("Age = 6.0"), lookup("age=6.0"));
        assert_eq!(lookup("Foo"), None);
        assert_eq!(lookup(""), None);
        assert_eq!(lookup("é"), None);
        assert_eq!(lookup("Age=6"), None);
    }

    #[test]
    fn sets_answer_what_cruby_measured() {
        let has = |name, c| contains(lookup(name).expect(name), c);
        assert!(has("Alpha", 'a') && !has("Alpha", '1'));
        assert!(has("Arabic", '\u{6E9}') && !has("Arabic", 'a'));
        assert!(has("Han", '松') && has("Hiragana", 'ま') && has("Katakana", 'ル'));
        assert!(has("Hangul", '루') && !has("Hangul", '('));
        assert!(has("Cn", '\u{10FFFF}') && !has("L", '\u{10FFFF}'));
    }

    /// An age holds every age before it: `U+1F600` came in 6.1, `U+1F601`
    /// in 6.0, `U+E000` in 1.1. Measured on CRuby 4.0.9.
    #[test]
    fn an_age_is_every_version_up_to_it() {
        let age = |v| lookup(v).expect(v);
        assert!(!contains(age("age=6.0"), '\u{1F600}'));
        assert!(contains(age("age=6.1"), '\u{1F600}'));
        assert!(contains(age("age=6.0"), '\u{1F601}'));
        assert!(contains(age("age=6.0"), '\u{E000}'));
        assert!(contains(age("age=17.0"), 'a'));
    }
}

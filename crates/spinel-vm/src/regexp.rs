//! `Regexp` and `MatchData` objects, and the table of compiled patterns.
//!
//! A compiled [`Regex`] is a Rust structure with no Ruby shape, so it does not
//! live in the heap. It lives here, in a per-heap table, and the `Regexp`
//! object holds an index into it — the same arrangement [`Definitions`] uses
//! for method bodies.
//!
//! The literal cache is the one part of this table the collector has to know
//! about. Ruby caches a regexp literal, so `2.times { rs << /foo/ }` pushes the
//! *same* object twice, and `regexp_spec.rb` checks it with `equal?`. A cached
//! object is reachable from nothing else, so [`Regexps::each_root`] hands it to
//! the marker.
//!
//! [`Definitions`]: crate::method::Definitions

use crate::bytecode::Iseq;
use crate::{Payload, Value};
use spinel_regex::{Flags, Regex};
use std::collections::HashMap;
use std::sync::Arc;

/// Slot layout of a `Regexp` object.
mod regexp_slot {
    /// Index into [`super::Regexps::compiled`].
    pub const INDEX: usize = 0;
    /// The source, as a `String` object, so `#source` is a slot read.
    pub const SOURCE: usize = 1;
    /// `Regexp#options`, as a fixnum.
    pub const OPTIONS: usize = 2;
    pub const COUNT: u32 = 3;
}

/// Slot layout of a `MatchData` object.
mod match_slot {
    pub const REGEXP: usize = 0;
    pub const SUBJECT: usize = 1;
    /// An `Array` of `2 * (groups + 1)` fixnums, `nil` where a group took no
    /// part. Character offsets, because that is what `MatchData#begin` answers.
    pub const OFFSETS: usize = 2;
    pub const COUNT: u32 = 3;
}

pub use match_slot::COUNT as MATCH_SLOTS;
pub use match_slot::{OFFSETS as MATCH_OFFSETS, REGEXP as MATCH_REGEXP, SUBJECT as MATCH_SUBJECT};
pub use regexp_slot::COUNT as REGEXP_SLOTS;
pub use regexp_slot::{INDEX as REGEXP_INDEX, OPTIONS as REGEXP_OPTIONS, SOURCE as REGEXP_SOURCE};

/// `Regexp::FIXEDENCODING`: the pattern matches only in its own encoding.
pub const FIXEDENCODING: i64 = 16;
/// `Regexp::NOENCODING`: the `/n` flag.
pub const NOENCODING: i64 = 32;

/// Where a regexp literal's options carry the encoding its `/u`, `/e` or `/s`
/// fixed it to, as the encoding's index plus one, so a literal and an
/// interpolation hand the constructor one integer. Never stored: the
/// constructor takes these bits off before `Regexp#options` can see them.
pub const LITERAL_ENCODING_SHIFT: u32 = 8;

/// Every pattern this heap has compiled, plus the literal cache.
#[derive(Default)]
pub struct Regexps {
    compiled: Vec<Arc<Regex>>,
    /// `(source, options)` to the one `Regexp` object a literal answers with.
    cache: HashMap<(String, i64), Value>,
    /// `/o` sites, keyed by the body they are in and their index within it.
    ///
    /// Not keyed by source: `/o` answers the *first* interpolation for ever, so
    /// two calls with different values share one object. The `Arc<Iseq>` is
    /// held rather than just its address — that is what makes the address a
    /// unique key, because a body this map refers to cannot be dropped and have
    /// another allocated where it was.
    once: HashMap<(usize, u32), (Arc<Iseq>, Value)>,
    /// Interned frozen Strings, by content (#21): what `"abc".freeze` and a
    /// literal under `# frozen_string_literal: true` answer, one object per
    /// content — CRuby's fstring table. Here because this is the heap's traced
    /// literal cache, and a frozen literal is a literal.
    ///
    /// The flag is whether the entry is a root. A literal's is: CRuby's iseq
    /// holds its fstring, and nothing here holds a literal's value but this
    /// table. One `String#-@` made is not (#384): the table is weak for it, as
    /// CRuby's is, and lets it go once nothing else references it.
    strings: HashMap<(Vec<u8>, u8), (Value, bool)>,
}

impl Regexps {
    #[must_use]
    pub fn new() -> Regexps {
        Regexps {
            compiled: Vec::new(),
            cache: HashMap::new(),
            once: HashMap::new(),
            strings: HashMap::new(),
        }
    }

    /// Compile `source` and keep it, answering the index the `Regexp` object
    /// stores.
    ///
    /// # Errors
    ///
    /// Whatever [`Regex::new`] refuses: a syntax error, or a construct the
    /// engine will not guess at.
    pub fn add(
        &mut self,
        source: &str,
        options: i64,
        encoding: spinel_regex::Encoding,
    ) -> Result<usize, spinel_regex::Error> {
        let regex = Regex::with_encoding(source, Flags::from_options(options), encoding)?;
        self.compiled.push(Arc::new(regex));
        Ok(self.compiled.len() - 1)
    }

    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Arc<Regex>> {
        self.compiled.get(index)
    }

    #[must_use]
    pub fn cached(&self, source: &str, options: i64) -> Option<Value> {
        self.cache.get(&(source.to_owned(), options)).copied()
    }

    pub fn cache(&mut self, source: &str, options: i64, value: Value) {
        self.cache.insert((source.to_owned(), options), value);
    }

    /// The cached literals, which are reachable from nothing else.
    pub fn each_root(&self, mut f: impl FnMut(Value)) {
        for value in self.cache.values() {
            f(*value);
        }
        for (_, value) in self.once.values() {
            f(*value);
        }
        for &(value, rooted) in self.strings.values() {
            if rooted {
                f(value);
            }
        }
    }

    /// The interned frozen String with these bytes, if one has been made.
    #[must_use]
    pub fn fstring(&self, bytes: &[u8], encoding: u8) -> Option<Value> {
        self.strings
            .get(&(bytes.to_vec(), encoding))
            .map(|&(value, _)| value)
    }

    /// The fstring a frozen literal with these bytes answers, if one has been
    /// made. One `String#-@` made becomes a root here: the literal refers to
    /// it from now on, as CRuby's iseq would.
    pub fn literal_fstring(&mut self, bytes: &[u8], encoding: u8) -> Option<Value> {
        let (value, rooted) = self.strings.get_mut(&(bytes.to_vec(), encoding))?;
        *rooted = true;
        Some(*value)
    }

    /// Make `value` the interned frozen String for its bytes and encoding:
    /// `"a"` in UTF-8 and in US-ASCII are two literals, and two objects (#19).
    pub fn intern_fstring(&mut self, bytes: &[u8], encoding: u8, value: Value) {
        self.strings
            .insert((bytes.to_vec(), encoding), (value, true));
    }

    /// The fstring for these bytes and encoding, making `value` it, held
    /// weakly, when there is none: what `String#-@` interns (#384).
    pub fn intern_weak_fstring(&mut self, bytes: &[u8], encoding: u8, value: Value) -> Value {
        self.strings
            .entry((bytes.to_vec(), encoding))
            .or_insert((value, false))
            .0
    }

    /// Let go of every weak fstring `live` says did not survive marking, so
    /// nothing reads a cell `sweep` is about to free.
    pub fn retain_live_fstrings(&mut self, live: impl Fn(Value) -> bool) {
        self.strings
            .retain(|_, &mut (value, rooted)| rooted || live(value));
    }

    /// What this `/o` site answered the first time it ran, if it has.
    #[must_use]
    pub fn once_cached(&self, iseq: &Arc<Iseq>, site: u32) -> Option<Value> {
        self.once
            .get(&(Arc::as_ptr(iseq) as usize, site))
            .map(|&(_, value)| value)
    }

    /// Record what this `/o` site answers from now on.
    pub fn cache_once(&mut self, iseq: &Arc<Iseq>, site: u32, value: Value) {
        self.once.insert(
            (Arc::as_ptr(iseq) as usize, site),
            (Arc::clone(iseq), value),
        );
    }
}

impl std::fmt::Debug for Regexps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Regexps")
            .field("compiled", &self.compiled.len())
            .field("cached", &self.cache.len())
            .finish()
    }
}

/// The payload and slot count a `Regexp` object is allocated with.
#[must_use]
pub const fn regexp_shape() -> (Payload, u32) {
    (Payload::Slots, REGEXP_SLOTS)
}

/// The payload and slot count a `MatchData` object is allocated with.
#[must_use]
pub const fn match_shape() -> (Payload, u32) {
    (Payload::Slots, MATCH_SLOTS)
}

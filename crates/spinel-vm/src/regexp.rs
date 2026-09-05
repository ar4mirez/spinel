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

/// Every pattern this heap has compiled, plus the literal cache.
#[derive(Default)]
pub struct Regexps {
    compiled: Vec<Arc<Regex>>,
    /// `(source, options)` to the one `Regexp` object a literal answers with.
    cache: HashMap<(String, i64), Value>,
}

impl Regexps {
    #[must_use]
    pub fn new() -> Regexps {
        Regexps {
            compiled: Vec::new(),
            cache: HashMap::new(),
        }
    }

    /// Compile `source` and keep it, answering the index the `Regexp` object
    /// stores.
    ///
    /// # Errors
    ///
    /// Whatever [`Regex::new`] refuses: a syntax error, or a construct the
    /// engine will not guess at.
    pub fn add(&mut self, source: &str, options: i64) -> Result<usize, spinel_regex::Error> {
        let regex = Regex::new(source, Flags::from_options(options))?;
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

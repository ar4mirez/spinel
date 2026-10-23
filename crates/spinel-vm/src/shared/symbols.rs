//! The symbol table: `SymbolId` ⇄ name, interned once for the process.
//!
//! [`Value::symbol`] has carried a `SymbolId` since
//! [#6](https://github.com/ar4mirez/spinel/issues/6) with nothing to give it a
//! name. Bytecode is what needs one: an `Iseq` stores symbols *by name* so it is
//! position-independent (see [`crate::bytecode`]), and linking one means turning
//! those names into ids.
//!
//! A name is bytes in an encoding, as a CRuby Symbol's is a String (#333):
//! equal bytes in two encodings are two Symbols unless both are ASCII, so
//! `"é".b.to_sym` and `:é` differ and `"a".b.to_sym` is `:a`. A name that is
//! ASCII in an ASCII-compatible encoding is kept as US-ASCII, which is what
//! `rb_str_intern` re-tags it to, so that one key stands for every spelling
//! CRuby's `rb_str_hash_cmp` would call equal.
//!
//! Symbols are never garbage collected in Ruby unless they were created
//! dynamically, and Spinel does not create them dynamically yet, so the table is
//! append-only in the strict sense: an id is an index that is never reused.
//!
//! [`Value::symbol`]: crate::Value::symbol

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use crate::strings::{US_ASCII, UTF_8};
use crate::value::SymbolId;

/// The name table. `names[id]` is the name and its encoding; `ids` is the
/// reverse index that makes interning idempotent, keyed by bytes and then by
/// encoding, so that a lookup borrows the bytes rather than building a key.
#[derive(Debug, Default)]
struct Table {
    names: Vec<(Box<[u8]>, u8)>,
    ids: HashMap<Box<[u8]>, Vec<(u8, SymbolId)>>,
}

impl Table {
    fn find(&self, bytes: &[u8], encoding: u8) -> Option<SymbolId> {
        self.ids
            .get(bytes)?
            .iter()
            .find(|&&(enc, _)| enc == encoding)
            .map(|&(_, id)| id)
    }
}

fn table() -> &'static RwLock<Table> {
    static TABLE: OnceLock<RwLock<Table>> = OnceLock::new();
    TABLE.get_or_init(RwLock::default)
}

/// A poisoned symbol table means another thread panicked mid-append. The table
/// is append-only, so the worst a panic can leave behind is a `names` entry with
/// no `ids` entry — one symbol interned twice, never a wrong name. Recovering is
/// therefore strictly better than propagating a panic into every later intern.
macro_rules! lock {
    (read) => {
        table()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    (write) => {
        table()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
}

/// The id for `name`, interning it if this is the first time it is seen.
///
/// Idempotent: the same name always gives the same id for the life of the
/// process. A Rust `str` is UTF-8, so this is the UTF-8 Symbol of that
/// name, US-ASCII when it is ASCII — what a literal in a UTF-8 file is.
#[must_use]
pub fn intern(name: &str) -> SymbolId {
    intern_bytes(name.as_bytes(), UTF_8)
}

/// The id for the name `bytes` in `encoding`, interning it if this is the
/// first time it is seen. Whether the bytes are characters of the encoding is
/// the caller's question: `String#to_sym` raises before it gets here.
#[must_use]
pub fn intern_bytes(bytes: &[u8], encoding: u8) -> SymbolId {
    // `rb_str_intern` re-tags an ASCII name as US-ASCII, and an empty name
    // hashes and compares equal to every other empty one.
    let encoding =
        if bytes.is_empty() || (crate::strings::ascii_compatible(encoding) && bytes.is_ascii()) {
            US_ASCII
        } else {
            encoding
        };

    // The read path is the common one — a compiler links the same names over and
    // over — so it does not take the write lock to find out the name is known.
    if let Some(id) = lock!(read).find(bytes, encoding) {
        return id;
    }

    let mut table = lock!(write);
    // Another thread may have interned it between the two locks.
    if let Some(id) = table.find(bytes, encoding) {
        return id;
    }

    // ponytail: u32 caps the process at 4 billion symbols and keeps `SymbolId`
    // inside `Value`'s tag. A program that reaches it has a symbol leak, which is
    // a bug worth a panic rather than a silent wrap.
    let id = SymbolId(u32::try_from(table.names.len()).expect("symbol table overflow"));
    let name: Box<[u8]> = bytes.into();
    table.names.push((name.clone(), encoding));
    table.ids.entry(name).or_default().push((encoding, id));
    id
}

/// `@__id__`, the hidden instance variable a class object carries its table id
/// in — memoised, because `class_of` reads it on every method call.
///
/// [`intern`] is a read lock plus a hash lookup, which measured as 4% of a
/// tight dispatch loop for six characters known at compile time. The memo lives
/// here rather than beside `class_of` for the reason `CLAUDE.md` gives: process-
/// global state belongs in this directory. It adds none that is not already
/// here — a `SymbolId` is an index into the append-only table this module owns.
#[must_use]
pub fn class_id_ivar() -> SymbolId {
    static ID: OnceLock<SymbolId> = OnceLock::new();
    *ID.get_or_init(|| intern(CLASS_ID_IVAR))
}

/// The name behind [`class_id_ivar`]. Public so `class.rs` can say what it is
/// reading without a second copy of the string.
pub const CLASS_ID_IVAR: &str = "@__id__";

/// The name `id` was interned under, or `None` if it was never interned.
///
/// Returns an owned `String` rather than a borrow: the table is behind a lock,
/// and nothing on a hot path asks for a name. This is the name as the VM
/// spells a method or a variable in a message; a name whose bytes are not
/// UTF-8 comes back with U+FFFD where they were. What a Ruby program sees is
/// [`bytes`], which `Symbol#to_s` reads.
#[must_use]
pub fn name(id: SymbolId) -> Option<String> {
    lock!(read)
        .names
        .get(id.0 as usize)
        .map(|(bytes, _)| String::from_utf8_lossy(bytes).into_owned())
}

/// The bytes and encoding `id` was interned under, or `None` if it was never
/// interned: US-ASCII for an ASCII name in any ASCII-compatible encoding.
#[must_use]
pub fn bytes(id: SymbolId) -> Option<(Vec<u8>, u8)> {
    lock!(read)
        .names
        .get(id.0 as usize)
        .map(|(bytes, encoding)| (bytes.to_vec(), *encoding))
}

/// How many symbols the process has interned.
#[must_use]
pub fn len() -> usize {
    lock!(read).names.len()
}

/// Every id interned so far, oldest first — `Symbol.all_symbols`.
///
/// An id is an index that is never reused, so the ids are `0..len()`; the
/// table is not read beyond its length.
#[must_use]
pub fn all() -> Vec<SymbolId> {
    let count = u32::try_from(len()).expect("symbol table overflow");
    (0..count).map(SymbolId).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent() {
        let first = intern("compile_me");
        let again = intern("compile_me");
        assert_eq!(first, again);
        assert_eq!(name(first).as_deref(), Some("compile_me"));
    }

    #[test]
    fn distinct_names_get_distinct_ids() {
        assert_ne!(intern("alpha_sym"), intern("beta_sym"));
    }

    #[test]
    fn an_id_that_was_never_interned_has_no_name() {
        assert_eq!(name(SymbolId(u32::MAX)), None);
    }

    #[test]
    fn a_symbol_survives_a_round_trip_through_a_value() {
        let id = intern("round_trip");
        let value = crate::Value::symbol(id);
        assert_eq!(value.as_symbol(), Some(id));
        assert_eq!(
            name(value.as_symbol().unwrap()).as_deref(),
            Some("round_trip")
        );
    }

    #[test]
    fn the_table_only_grows() {
        // The property, stated so that a test running in parallel cannot
        // disturb it: this table is process-global, so a `len()` read before
        // and after would race any other test that interns — and since
        // `bootstrap` interns the primitives' names, most of them do.
        let first = intern("grow_one");
        assert_eq!(intern("grow_one"), first, "interning is idempotent");
        assert_ne!(intern("grow_two"), first, "a new name gets a new id");
        assert_eq!(name(first).as_deref(), Some("grow_one"));
        // Ids are indices into an append-only table, so one handed out earlier
        // stays valid however much the table has grown since.
        assert!(len() > first.0 as usize);
    }

    #[test]
    fn equal_bytes_in_two_encodings_are_two_symbols() {
        let utf8 = intern("caf\u{e9}_two");
        let binary = intern_bytes("caf\u{e9}_two".as_bytes(), crate::strings::BINARY);
        assert_ne!(utf8, binary);
        assert_eq!(
            bytes(binary),
            Some(("caf\u{e9}_two".as_bytes().to_vec(), crate::strings::BINARY))
        );
        assert_eq!(bytes(utf8).map(|(_, enc)| enc), Some(UTF_8));
    }

    #[test]
    fn an_ascii_name_is_one_us_ascii_symbol_in_every_compatible_encoding() {
        let plain = intern("ascii_everywhere");
        assert_eq!(
            intern_bytes(b"ascii_everywhere", crate::strings::BINARY),
            plain
        );
        assert_eq!(bytes(plain).map(|(_, enc)| enc), Some(US_ASCII));
        // UTF-16LE is not ASCII-compatible, so its "a" is a Symbol of its own.
        let utf16 = crate::strings::index_named("UTF-16LE").expect("UTF-16LE");
        assert_ne!(intern_bytes(b"a\0s\0", utf16), intern("a\0s\0"));
    }

    #[test]
    fn a_name_that_is_not_utf8_has_a_lossy_rust_name() {
        let id = intern_bytes(b"\xff_lossy", crate::strings::BINARY);
        assert_eq!(name(id).as_deref(), Some("\u{fffd}_lossy"));
    }

    #[test]
    fn all_names_every_interned_id() {
        let id = intern("listed_by_all");
        assert!(all().contains(&id));
    }
}

/// A compiled unit's symbols as ids, resolved the first time a frame runs it.
///
/// It lives here because it is this table's answer, kept: the table is one per
/// process and only grows, so a name's id is the same for as long as the
/// process lives and one resolution serves every call. It holds ids and no
/// `Value`, and nothing can change it once set.
///
/// It is not part of what the unit *is* — two units with the same
/// instructions are equal whether or not either has run, and a clone starts
/// unresolved — so equality ignores it.
#[derive(Debug, Default)]
pub struct Linked(OnceLock<std::sync::Arc<[SymbolId]>>);

impl Linked {
    /// The ids, from `resolve` the first time and from memory after.
    pub fn get_or_resolve(
        &self,
        resolve: impl FnOnce() -> Vec<SymbolId>,
    ) -> std::sync::Arc<[SymbolId]> {
        std::sync::Arc::clone(self.0.get_or_init(|| resolve().into()))
    }
}

impl Clone for Linked {
    fn clone(&self) -> Linked {
        Linked::default()
    }
}

impl PartialEq for Linked {
    fn eq(&self, _: &Linked) -> bool {
        true
    }
}

//! The symbol table: `SymbolId` ⇄ name, interned once for the process.
//!
//! [`Value::symbol`] has carried a `SymbolId` since
//! [#6](https://github.com/ar4mirez/spinel/issues/6) with nothing to give it a
//! name. Bytecode is what needs one: an `Iseq` stores symbols *by name* so it is
//! position-independent (see [`crate::bytecode`]), and linking one means turning
//! those names into ids.
//!
//! Symbols are never garbage collected in Ruby unless they were created
//! dynamically, and Spinel does not create them dynamically yet, so the table is
//! append-only in the strict sense: an id is an index that is never reused.
//!
//! [`Value::symbol`]: crate::Value::symbol

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use crate::value::SymbolId;

/// The name table. `names[id]` is the name; `ids` is the reverse index that
/// makes interning idempotent.
#[derive(Debug, Default)]
struct Table {
    names: Vec<Box<str>>,
    ids: HashMap<Box<str>, SymbolId>,
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
/// process.
#[must_use]
pub fn intern(name: &str) -> SymbolId {
    // The read path is the common one — a compiler links the same names over and
    // over — so it does not take the write lock to find out the name is known.
    if let Some(&id) = lock!(read).ids.get(name) {
        return id;
    }

    let mut table = lock!(write);
    // Another thread may have interned it between the two locks.
    if let Some(&id) = table.ids.get(name) {
        return id;
    }

    // ponytail: u32 caps the process at 4 billion symbols and keeps `SymbolId`
    // inside `Value`'s tag. A program that reaches it has a symbol leak, which is
    // a bug worth a panic rather than a silent wrap.
    let id = SymbolId(u32::try_from(table.names.len()).expect("symbol table overflow"));
    let name: Box<str> = name.into();
    table.names.push(name.clone());
    table.ids.insert(name, id);
    id
}

/// The name `id` was interned under, or `None` if it was never interned.
///
/// Returns an owned `String` rather than a borrow: the table is behind a lock,
/// and nothing on a hot path asks for a name. `Symbol#to_s` in phase 2 allocates
/// a Ruby `String` here anyway.
#[must_use]
pub fn name(id: SymbolId) -> Option<String> {
    lock!(read).names.get(id.0 as usize).map(|n| n.to_string())
}

/// How many symbols the process has interned.
#[must_use]
pub fn len() -> usize {
    lock!(read).names.len()
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
        let before = len();
        let _ = intern("grow_one");
        let _ = intern("grow_one");
        let _ = intern("grow_two");
        assert_eq!(len(), before + 2);
    }
}

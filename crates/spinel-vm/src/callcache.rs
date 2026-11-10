//! Monomorphic inline caches, one entry per call site.
//!
//! `docs/engine.md`: "Call sites carry a monomorphic inline cache
//! `(class serial, target)`. Because bytecode is shared across Ractors, inline
//! caches do not live in the bytecode; each heap owns a side table indexed by
//! call-site id."
//!
//! This is that side table. It sits in front of [`Classes::lookup`], which is
//! itself a memo in front of the chain walk — so the thing to beat is not the
//! walk but the one hash probe the per-class cache costs. That is why the hot
//! path here is a `Vec` index and two integer comparisons, and why the one
//! hash probe this design needs happens on frame entry instead.
//!
//! [`Classes::lookup`]: crate::class::Classes::lookup

use std::collections::HashMap;
use std::sync::Arc;

use crate::bytecode::Iseq;
use crate::class::{ClassId, Method};
use crate::shape::ShapeId;

/// What one call site last resolved, and what has to still be true to reuse it.
///
/// `Copy`, and three words: the guard reads the whole entry, so splitting the
/// key from the payload would only cost a second index.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// The receiver's class when this was filled.
    class: ClassId,
    /// `class`'s serial then. See [`Classes::serial`] — bumped by anything that
    /// can change what a name resolves to *for that class*, which is exactly
    /// the set that can invalidate this entry.
    ///
    /// [`Classes::serial`]: crate::class::Classes::serial
    serial: u64,
    method: Method,
}

/// What one [`Insn::GetIvar`] site last read: the receiver's shape and where
/// that shape keeps the site's name.
///
/// Needs no serial. A shape is a fixed path of names, so the index a shape
/// gives a name never changes; and a site reads one name, fixed by the
/// instruction. `shape` is [`ShapeId::NONE`] while the site has read nothing,
/// which no object holding an instance variable wears.
///
/// [`Insn::GetIvar`]: crate::bytecode::Insn::GetIvar
#[derive(Debug, Clone, Copy)]
pub struct IvarEntry {
    pub shape: ShapeId,
    pub index: u16,
}

impl IvarEntry {
    const EMPTY: IvarEntry = IvarEntry {
        shape: ShapeId::NONE,
        index: 0,
    };
}

/// One heap's inline caches.
///
/// Per heap because a [`Method`] names per-heap tables and a `ClassId` indexes
/// one heap's class list, while the `Iseq` they were resolved from is shared.
pub struct CallCaches {
    /// One slot per call site of every `Iseq` this heap has entered, in runs.
    entries: Vec<Option<Entry>>,
    /// The instance-variable sites' caches, indexed as `entries` is: an
    /// `Iseq`'s run there is its call sites and then its
    /// [`Iseq::ivar_sites`], so each table has a slot the other never reads.
    /// Four bytes a call site is cheaper than a second base per frame.
    ivars: Vec<IvarEntry>,
    /// Where each `Iseq`'s run starts, by `Arc` address.
    ///
    /// An address is only a safe key because `iseqs` below holds a clone of the
    /// same `Arc`: the `Iseq` cannot be dropped while this map remembers it, so
    /// its address cannot be reused by a different one. `Definitions::intern_iseq`
    /// keys its memo the same way for the same reason. Storing a base without
    /// keeping the `Arc` would eventually hand a new `Iseq` another one's
    /// entries, which is a wrong-method bug rather than a stale-answer one.
    bases: HashMap<usize, u32>,
    /// The clones that make `bases` sound.
    ///
    /// ponytail: an `Iseq` this heap has entered is kept alive until the heap
    /// goes. `Definitions` already holds every method and block body for the
    /// same span, so this adds a reference to things that were staying anyway;
    /// the one it adds is the top-level script's. Give the runs back when
    /// something can actually drop an `Iseq` mid-run.
    iseqs: Vec<Arc<Iseq>>,
    /// The bases `base` answered last, direct-mapped by `Arc` address, in
    /// front of the `bases` probe: a recursive or loop-called method enters
    /// the same `Iseq` over and over. Sound for the reason `bases` is.
    recent: [(usize, u32); RECENT],
}

/// How many bases [`CallCaches::base`] remembers without a probe.
const RECENT: usize = 64;

impl Default for CallCaches {
    fn default() -> CallCaches {
        CallCaches {
            entries: Vec::new(),
            ivars: Vec::new(),
            bases: HashMap::new(),
            iseqs: Vec::new(),
            recent: [(0, 0); RECENT],
        }
    }
}

impl CallCaches {
    #[must_use]
    pub fn new() -> CallCaches {
        CallCaches::default()
    }

    /// The first slot of `iseq`'s run, allocating the run the first time this
    /// heap sees it.
    ///
    /// Called once per frame push, beside `Iseq::link`, rather than once per
    /// send: the hash probe belongs on frame entry, where `link` already pays
    /// one per symbol, and not on the path this table exists to shorten.
    ///
    /// # Panics
    ///
    /// If one heap ever holds more call sites than a `u32` can index.
    pub fn base(&mut self, iseq: &Arc<Iseq>) -> u32 {
        let key = Arc::as_ptr(iseq) as usize;
        // An `Iseq` is far bigger than 64 bytes, so the bits above them tell
        // two apart.
        let recent = (key >> 6) % RECENT;
        if self.recent[recent].0 == key {
            return self.recent[recent].1;
        }
        if let Some(&base) = self.bases.get(&key) {
            self.recent[recent] = (key, base);
            return base;
        }
        let base = u32::try_from(self.entries.len()).expect("a heap under 2^32 call sites");
        let run = iseq.call_sites.len() + iseq.ivar_sites as usize;
        self.entries.resize(self.entries.len() + run, None);
        self.ivars.resize(self.entries.len(), IvarEntry::EMPTY);
        self.bases.insert(key, base);
        self.iseqs.push(Arc::clone(iseq));
        self.recent[recent] = (key, base);
        base
    }

    /// What `slot` resolved to, if it resolved to something for this exact
    /// class and that class has not changed since.
    ///
    /// # Panics
    ///
    /// If `slot` is outside the run [`CallCaches::base`] allocated, which means
    /// a caller added an operand to a base from a different `Iseq`.
    #[must_use]
    pub fn get(&self, slot: u32, class: ClassId, serial: u64) -> Option<Method> {
        match self.entries[slot as usize] {
            Some(entry) if entry.class == class && entry.serial == serial => Some(entry.method),
            _ => None,
        }
    }

    /// Memoise what a full lookup just answered.
    ///
    /// Monomorphic: a second receiver class at the same site overwrites rather
    /// than extending. A polymorphic site pays the probe it would have paid
    /// anyway, plus the guard.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    pub fn fill(&mut self, slot: u32, class: ClassId, serial: u64, method: Method) {
        self.entries[slot as usize] = Some(Entry {
            class,
            serial,
            method,
        });
    }

    /// What the instance-variable site at `slot` last read.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    #[must_use]
    pub fn ivar(&self, slot: u32) -> IvarEntry {
        self.ivars[slot as usize]
    }

    /// Every instance-variable cache, for a reader that will index it with
    /// slots from [`CallCaches::base`] while nothing fills one.
    #[must_use]
    pub fn ivars(&self) -> &[IvarEntry] {
        &self.ivars
    }

    /// Remember that the site at `slot` found its name at `index` of an
    /// object wearing `shape`. Monomorphic, as [`CallCaches::fill`] is.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    pub fn fill_ivar(&mut self, slot: u32, shape: ShapeId, index: u16) {
        debug_assert!(shape != ShapeId::NONE, "a cached read of nothing");
        self.ivars[slot as usize] = IvarEntry { shape, index };
    }

    /// How many call sites are currently memoised. For tests, the way
    /// [`Classes::cached_lookups`] is.
    ///
    /// [`Classes::cached_lookups`]: crate::class::Classes::cached_lookups
    #[must_use]
    pub fn filled(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    /// How many slots have been handed out. For tests and for the bench.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Builtin;
    use crate::bytecode::{BlockRef, CallSite};
    use crate::class::Visibility;
    use crate::heap::Heap;
    use crate::value::Value;

    fn booted() -> Heap {
        let mut heap = Heap::new();
        heap.scope().bootstrap();
        heap
    }

    /// An `Iseq` with `sites` call sites and nothing else. Only the count
    /// matters here: this table never reads a site, it only counts them.
    fn iseq_with(sites: usize) -> Arc<Iseq> {
        let site = CallSite {
            name: 0,
            argc: 0,
            splats: Vec::new(),
            keywords: Vec::new(),
            block: BlockRef::None,
            implicit_self: false,
            variable_call: false,
            kwsplat: false,
        };
        Arc::new(Iseq {
            call_sites: vec![site; sites],
            ..Iseq::default()
        })
    }

    /// The runs do not overlap, and asking twice for the same `Iseq` answers
    /// the same base — the memo, without which every frame push would hand the
    /// same call site a different slot.
    #[test]
    fn each_iseq_gets_its_own_run_once() {
        let mut caches = CallCaches::new();
        let first = iseq_with(3);
        let second = iseq_with(2);

        assert_eq!(caches.base(&first), 0);
        assert_eq!(caches.base(&second), 3);
        assert_eq!(caches.base(&first), 0, "the same `Iseq` keeps its run");
        assert_eq!(caches.len(), 5);
    }

    /// A body's run holds its instance-variable sites after its call sites,
    /// so the next body's run starts past both, and an ivar slot reads back
    /// what was filled into it and nothing before.
    #[test]
    fn an_iseqs_run_holds_its_ivar_sites_after_its_call_sites() {
        let mut caches = CallCaches::new();
        let first = Arc::new(Iseq {
            ivar_sites: 3,
            ..(*iseq_with(2)).clone()
        });
        let base = caches.base(&first);
        assert_eq!(caches.base(&iseq_with(1)), base + 5);

        let slot = base + 2 + 1;
        assert_eq!(caches.ivar(slot).shape, ShapeId::NONE, "nothing read yet");
        caches.fill_ivar(slot, ShapeId(7), 4);
        let entry = caches.ivar(slot);
        assert_eq!((entry.shape, entry.index), (ShapeId(7), 4));
        assert_eq!(caches.ivar(slot - 1).shape, ShapeId::NONE);
        assert_eq!(caches.filled(), 0, "an ivar slot is not a call site");
    }

    /// The invariant that makes an address a legal key: the table holds a clone,
    /// so the `Iseq` cannot be dropped while `bases` remembers where it points.
    ///
    /// Without this, a freed `Iseq`'s address could be handed to a new one, and
    /// the new one would inherit the old one's entries — a wrong-method bug,
    /// and one that no guard here would catch, because the entries would be
    /// perfectly valid for a call site that no longer exists.
    #[test]
    fn the_table_keeps_the_iseq_its_key_points_at_alive() {
        let mut caches = CallCaches::new();
        let iseq = iseq_with(2);
        let base = caches.base(&iseq);
        assert_eq!(Arc::strong_count(&iseq), 2, "the table took a clone");

        drop(iseq);
        // The run is still addressable, and the address is still spoken for.
        assert_eq!(caches.len(), 2);
        assert_eq!(base, 0);
    }

    /// A hit: fill a slot, and read it back through a guard that agrees.
    #[test]
    fn a_filled_site_answers_the_class_it_was_filled_for() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = crate::shared::symbols::intern("inline_cache_hit");
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        scope
            .classes_mut()
            .define_method(c, name, Value::fixnum(1).unwrap());
        let method = scope.classes_mut().lookup(c, name).unwrap();
        let serial = scope.classes().serial(c);

        let mut caches = CallCaches::new();
        let base = caches.base(&iseq_with(1));
        assert_eq!(caches.filled(), 0);
        assert_eq!(
            caches.get(base, c, serial),
            None,
            "empty before it is filled"
        );

        caches.fill(base, c, serial, method);
        assert_eq!(caches.filled(), 1);
        assert_eq!(caches.get(base, c, serial).unwrap().owner, method.owner);
    }

    /// Guard failure 1: a different receiver class at the same site. This is
    /// also what `def obj.foo` looks like from here — a singleton *replaces*
    /// the object's class, so the next send sees a different `ClassId`.
    #[test]
    fn a_different_receiver_class_misses() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = crate::shared::symbols::intern("inline_cache_class_change");
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        let d = scope.define_class(Some("D"), Some(Builtin::Object.id()));
        scope
            .classes_mut()
            .define_method(c, name, Value::fixnum(1).unwrap());
        scope
            .classes_mut()
            .define_method(d, name, Value::fixnum(2).unwrap());
        let from_c = scope.classes_mut().lookup(c, name).unwrap();

        let mut caches = CallCaches::new();
        let base = caches.base(&iseq_with(1));
        caches.fill(base, c, scope.classes().serial(c), from_c);

        assert!(
            caches.get(base, d, scope.classes().serial(d)).is_none(),
            "`C`'s answer is not `D`'s"
        );
        // And re-filling for `D` replaces rather than extends: monomorphic.
        let from_d = scope.classes_mut().lookup(d, name).unwrap();
        caches.fill(base, d, scope.classes().serial(d), from_d);
        assert_eq!(caches.filled(), 1);
        assert_eq!(
            caches
                .get(base, d, scope.classes().serial(d))
                .unwrap()
                .owner,
            d
        );
        assert!(caches.get(base, c, scope.classes().serial(c)).is_none());
    }

    /// Narrowing a method's visibility must invalidate a warm site (#161).
    ///
    /// Visibility lives on the memoised `Method`, so `private :m` on a method
    /// some site has already called has to bump the serial the way a
    /// redefinition does. Without it a site that called `obj.m` while it was
    /// public keeps calling it — and so does every already-warm site in the
    /// heap, which would look intermittent and receiver-dependent rather than
    /// reproducible.
    #[test]
    fn narrowing_visibility_misses() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = crate::shared::symbols::intern("inline_cache_visibility_change");
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        scope
            .classes_mut()
            .define_method(c, name, Value::fixnum(1).unwrap());
        let public = scope.classes_mut().lookup(c, name).unwrap();
        assert_eq!(public.visibility, Visibility::Public);

        let mut caches = CallCaches::new();
        let base = caches.base(&iseq_with(1));
        let serial = scope.classes().serial(c);
        caches.fill(base, c, serial, public);
        assert!(caches.get(base, c, serial).is_some());

        assert!(
            scope
                .classes_mut()
                .set_visibility(c, name, Visibility::Private)
        );
        let bumped = scope.classes().serial(c);
        assert!(bumped > serial, "`private :m` bumps `C`'s serial");
        assert!(
            caches.get(base, c, bumped).is_none(),
            "the stale entry still says the method is public"
        );
        assert_eq!(
            scope.classes_mut().lookup(c, name).unwrap().visibility,
            Visibility::Private,
            "and the lookup that re-fills carries the new visibility"
        );
    }

    /// Guard failure 2: the class is right and its serial moved. A definition
    /// on an ancestor is the case that a class check alone would get wrong —
    /// the receiver class never changes, and the method it resolves to does.
    #[test]
    fn a_bumped_serial_misses() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = crate::shared::symbols::intern("inline_cache_serial_change");
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        scope
            .classes_mut()
            .define_method(Builtin::Object.id(), name, Value::fixnum(1).unwrap());
        let inherited = scope.classes_mut().lookup(c, name).unwrap();
        assert_eq!(inherited.owner, Builtin::Object.id());

        let mut caches = CallCaches::new();
        let base = caches.base(&iseq_with(1));
        let serial = scope.classes().serial(c);
        caches.fill(base, c, serial, inherited);
        assert!(caches.get(base, c, serial).is_some());

        // An override on `C` itself: same receiver class, different answer.
        scope
            .classes_mut()
            .define_method(c, name, Value::fixnum(2).unwrap());
        let bumped = scope.classes().serial(c);
        assert!(bumped > serial, "defining on `C` bumps `C`'s serial");
        assert!(
            caches.get(base, c, bumped).is_none(),
            "the stale entry still names `Object`'s method"
        );

        let now = scope.classes_mut().lookup(c, name).unwrap();
        assert_eq!(now.owner, c, "and the full lookup that re-fills is right");
    }

    /// A definition somewhere unrelated leaves the entry alone. This is the
    /// whole point of guarding on a per-class serial rather than a global one:
    /// #9 bought precision on the write side, and a global stamp here would
    /// give it back.
    #[test]
    fn an_unrelated_definition_leaves_the_entry_alone() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = crate::shared::symbols::intern("inline_cache_unrelated");
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        let elsewhere = scope.define_class(Some("Elsewhere"), Some(Builtin::Object.id()));
        scope
            .classes_mut()
            .define_method(c, name, Value::fixnum(1).unwrap());
        let method = scope.classes_mut().lookup(c, name).unwrap();

        let mut caches = CallCaches::new();
        let base = caches.base(&iseq_with(1));
        let serial = scope.classes().serial(c);
        caches.fill(base, c, serial, method);

        scope.classes_mut().define_method(
            elsewhere,
            crate::shared::symbols::intern("inline_cache_noise"),
            Value::fixnum(9).unwrap(),
        );
        assert_eq!(scope.classes().serial(c), serial, "`C` did not move");
        assert!(
            caches.get(base, c, scope.classes().serial(c)).is_some(),
            "so its entry is still good"
        );
    }
}

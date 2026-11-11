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
use std::ptr::NonNull;
use std::sync::Arc;

use crate::bytecode::Iseq;
use crate::class::{ClassId, CrefId, Method};
use crate::method::Native;
use crate::shape::ShapeId;
use crate::value::{SymbolId, Value};

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

/// What an [`Insn::GetConst`] site found last time: the value, for a read
/// from that lexical scope against that module, at that constant epoch (see
/// `Classes::const_epoch`).
///
/// The epoch is the whole guard: everything that can change what a read
/// answers moves it, so an entry from an older epoch is never believed. Nor
/// is one's value ever read then, which is why a stale entry may go on
/// holding a value the collector has since freed — it is a word compared
/// with nothing, as a stale call-cache entry's body is — while a current
/// entry's value is still in the constant table it came from, which roots
/// it. The scope and the module are in the key because one site can run
/// under more than one: a body `class_eval`'d into two classes reads
/// lexically from both, and `A::X` reads from whatever `A` is.
///
/// [`Insn::GetConst`]: crate::bytecode::Insn::GetConst
#[derive(Debug, Clone, Copy)]
pub struct ConstEntry {
    pub epoch: u64,
    pub cref: CrefId,
    pub from: ClassId,
    pub value: Value,
}

/// How a call from one site binds the method its entry holds, worked out the
/// first time the entry answers a call and kept until the entry is refilled
/// — CRuby's `vm_callee_setup_arg` deciding once per call site that a call
/// can take its fast path, and its keyword half matching the site's keyword
/// names to the callee's once rather than per call.
///
/// It needs no guard of its own. The entry's class and serial already
/// pin which method answers, so the method's body is fixed while the entry
/// is valid, and the site is fixed by its slot; [`CallCaches::fill`]
/// forgets the plan along with the entry it replaces.
#[derive(Debug, Clone, Default)]
pub enum Plan {
    /// Not worked out yet: the entry has not answered a call since it was
    /// filled.
    #[default]
    Unknown,
    /// The call takes the general path, every time this entry answers it.
    General,
    /// The call binds on the operand stack: see [`Direct`].
    Direct(Arc<Direct>),
    /// A `yield` site's: the block body it last yielded to, and whether a
    /// yield from it binds on the stack. See [`Yielded`].
    Yielded(Arc<Yielded>),
    /// The call is to a primitive that answers from the operand stack, for
    /// the cases it can, without a `Pending`: see [`NativePlan`].
    Native(NativePlan),
}

/// A call to a primitive with a light form: one that takes its receiver and
/// arguments where they stand on the operand stack and answers the cases it
/// can — `String#<<` of a String it can append in place, for one — leaving
/// every other to the general path, which then decides them as it always
/// has. Made only for a public method, a site with positional arguments
/// alone, and a receiver class `known_class_of` answers.
#[derive(Debug, Clone, Copy)]
pub struct NativePlan {
    /// The primitive.
    pub native: Native,
    /// The receiver class the entry holds the method for.
    pub class: ClassId,
    /// `class`'s object, the guard on the receiver's header, as
    /// [`Direct::class_object`] is.
    pub class_object: Value,
    /// The name the site calls, which a refinement is looked up by.
    pub name: SymbolId,
    /// How many arguments the site passes, above its receiver.
    pub argc: u16,
}

/// An `Iseq` this heap has entered, held by a frame without a count of its
/// own: a pointer, where an `Arc` clone would cost every call and every
/// return an atomic increment and decrement of a count shared across
/// Ractors.
///
/// Only [`CallCaches::enter`] makes one, and that is what makes it sound:
/// the heap's caches keep a clone of the `Arc` for as long as the heap lives
/// (see `CallCaches::iseqs`), never letting one go, and an `Iseq` behind an
/// `Arc` is never written. So the `Iseq` outlives every frame of the heap,
/// all of which live in the heap's own loop and fibers, and a shared
/// reference to it is valid whenever one is read. It is a frame's to hold,
/// not to keep past its heap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entered(NonNull<Iseq>);

// SAFETY: it is a shared reference to an `Iseq`, which is `Sync` — `Arc<Iseq>`
// already crosses threads — and nothing is written through it.
unsafe impl Send for Entered {}
// SAFETY: as for `Send`.
unsafe impl Sync for Entered {}

impl std::ops::Deref for Entered {
    type Target = Iseq;

    fn deref(&self) -> &Iseq {
        // SAFETY: made by `CallCaches::enter` from an `Arc` the caches keep
        // for the life of the heap, as the type's documentation says, and
        // never written through.
        unsafe { self.0.as_ref() }
    }
}

impl Entered {
    /// An owning clone, for something that may outlive the frame — a
    /// `Binding`, a `Proc` — or holds one while frames change.
    #[must_use]
    pub fn to_arc(self) -> Arc<Iseq> {
        let pointer = self.0.as_ptr().cast_const();
        // SAFETY: the pointer came from `Arc::as_ptr` on an `Arc` the caches
        // still hold, so the allocation is live and one strong count is
        // already the caches'; adding one and taking it as an `Arc` is the
        // clone that `Arc` would make.
        unsafe {
            Arc::increment_strong_count(pointer);
            Arc::from_raw(pointer)
        }
    }
}

/// What a `yield` site worked out for the block body it last yielded to.
///
/// A yield site resolves no method, so its entry is never filled and nothing
/// forgets this; what it knows follows from the body alone — a definition
/// id, which names one `Iseq` for the life of the heap — and the site, so
/// the body is its whole guard. A site that yields to another body works it
/// out again.
#[derive(Debug)]
pub struct Yielded {
    /// The `Proc`'s body, as its `PROC_BODY` slot holds it.
    pub body: Value,
    /// Whether a yield from this site to this body binds on the stack:
    /// what `yield_simple` checks of the body's parameters.
    pub fits: bool,
    /// The body.
    pub iseq: Entered,
    /// [`CallCaches::base`] for it.
    pub cache_base: u32,
}

/// A call whose arguments are bound where they already are, on the operand
/// stack: positional arguments into the callee's first locals, each keyword
/// into its parameter's slot, and the parameters the site leaves out to the
/// undef marker the body's guarded defaults look for.
#[derive(Debug)]
pub struct Direct {
    /// The callee's body.
    pub iseq: Entered,
    /// Where the callee's own run of entries starts: [`CallCaches::base`]
    /// for `iseq`, asked once here rather than at every frame push.
    pub cache_base: u32,
    /// The name the site calls, which the frame is entered under.
    pub name: SymbolId,
    /// The receiver class the entry holds the method for.
    pub class: ClassId,
    /// `class`'s object, which a receiver whose header names it is an
    /// instance of: the site's guard, short of the class id. Never `Class`'s,
    /// the one class that does not follow from the header — a singleton
    /// class's own singleton may be due — since `known_class_of` never
    /// answers it and no plan is made without that answer.
    pub class_object: Value,
    /// For each keyword the site writes, in the site's order, the callee
    /// local it binds.
    pub keywords: Box<[u16]>,
    /// Whether those locals are the ones right after the positionals, in
    /// the site's order: every argument is then already where its local
    /// goes, once the receiver below them is gone.
    pub in_order: bool,
    /// The optional parameters, positional or keyword, that a call from this
    /// site does not supply.
    pub undef: Box<[u16]>,
}

/// One heap's inline caches.
///
/// Per heap because a [`Method`] names per-heap tables and a `ClassId` indexes
/// one heap's class list, while the `Iseq` they were resolved from is shared.
pub struct CallCaches {
    /// One slot per call site of every `Iseq` this heap has entered, in runs.
    entries: Vec<Option<Entry>>,
    /// How the call each entry answers binds, indexed as `entries` is.
    plans: Vec<Plan>,
    /// The instance-variable sites' caches, indexed as `entries` is: an
    /// `Iseq`'s run there is its call sites and then its
    /// [`Iseq::ivar_sites`], so each table has a slot the other never reads.
    /// Four bytes a call site is cheaper than a second base per frame.
    ivars: Vec<IvarEntry>,
    /// The constant sites' caches, indexed as `entries` is, for the same
    /// reason: an `Iseq`'s run there is its call sites, its ivar sites and
    /// then its [`Iseq::const_sites`].
    consts: Vec<Option<ConstEntry>>,
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
            plans: Vec::new(),
            ivars: Vec::new(),
            consts: Vec::new(),
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
        self.enter(iseq).0
    }

    /// [`CallCaches::base`], and the [`Entered`] a frame running `iseq`
    /// holds it by.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::base`].
    pub fn enter(&mut self, iseq: &Arc<Iseq>) -> (u32, Entered) {
        // From `Arc::as_ptr` rather than a reference to the `Iseq`: its
        // provenance is the whole allocation, counts included, which
        // `Entered::to_arc` steps back into.
        let pointer = Arc::as_ptr(iseq);
        let entered = Entered(NonNull::new(pointer.cast_mut()).expect("an `Arc` is never null"));
        let key = pointer as usize;
        // An `Iseq` is far bigger than 64 bytes, so the bits above them tell
        // two apart.
        let recent = (key >> 6) % RECENT;
        if self.recent[recent].0 == key {
            return (self.recent[recent].1, entered);
        }
        if let Some(&base) = self.bases.get(&key) {
            self.recent[recent] = (key, base);
            return (base, entered);
        }
        let base = u32::try_from(self.entries.len()).expect("a heap under 2^32 call sites");
        let run = iseq.call_sites.len() + iseq.ivar_sites as usize + iseq.const_sites as usize;
        self.entries.resize(self.entries.len() + run, None);
        self.plans.resize(self.entries.len(), Plan::Unknown);
        self.ivars.resize(self.entries.len(), IvarEntry::EMPTY);
        self.consts.resize(self.entries.len(), None);
        self.bases.insert(key, base);
        self.iseqs.push(Arc::clone(iseq));
        self.recent[recent] = (key, base);
        (base, entered)
    }

    /// What `slot` resolved to, if it resolved to something for this exact
    /// class and that class has not changed since.
    ///
    /// # Panics
    ///
    /// If `slot` is outside the run [`CallCaches::base`] allocated, which means
    /// a caller added an operand to a base from a different `Iseq`.
    #[must_use]
    #[inline]
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
        self.plans[slot as usize] = Plan::Unknown;
    }

    /// How a call from `slot` binds the method its entry holds, if that has
    /// been worked out since the entry was filled.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    #[must_use]
    #[inline]
    pub fn plan(&self, slot: u32) -> &Plan {
        &self.plans[slot as usize]
    }

    /// Remember how a call from `slot` binds, until the entry is refilled.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    pub fn set_plan(&mut self, slot: u32, plan: Plan) {
        self.plans[slot as usize] = plan;
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

    /// What the [`Insn::GetConst`] site at `slot` found last, if it found
    /// something at epoch `epoch`, reading from `cref` against `from`.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    ///
    /// [`Insn::GetConst`]: crate::bytecode::Insn::GetConst
    #[inline]
    #[must_use]
    pub fn constant(&self, slot: u32, epoch: u64, cref: CrefId, from: ClassId) -> Option<Value> {
        let entry = self.consts[slot as usize]?;
        (entry.epoch == epoch && entry.cref == cref && entry.from == from).then_some(entry.value)
    }

    /// [`CallCaches::constant`] for a site that reads lexically or from the
    /// top level, whose module follows from `cref` alone, so is not asked.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    #[inline]
    #[must_use]
    pub fn scoped_constant(&self, slot: u32, epoch: u64, cref: CrefId) -> Option<Value> {
        let entry = self.consts[slot as usize]?;
        (entry.epoch == epoch && entry.cref == cref).then_some(entry.value)
    }

    /// Remember what the constant site at `slot` found. Monomorphic, as
    /// [`CallCaches::fill`] is.
    ///
    /// # Panics
    ///
    /// As [`CallCaches::get`].
    pub fn fill_constant(&mut self, slot: u32, entry: ConstEntry) {
        self.consts[slot as usize] = Some(entry);
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

    /// A body's constant sites come after its ivar sites in its run, and an
    /// entry answers only for the epoch, scope and module it was filled for.
    #[test]
    fn a_constant_entry_answers_only_for_its_epoch_scope_and_module() {
        let mut caches = CallCaches::new();
        let body = Arc::new(Iseq {
            ivar_sites: 2,
            const_sites: 3,
            ..(*iseq_with(1)).clone()
        });
        let base = caches.base(&body);
        assert_eq!(caches.base(&iseq_with(1)), base + 6);

        let slot = base + 1 + 2 + 1;
        let (scope, other_scope) = (CrefId::from_index(0), CrefId::from_index(1));
        let (module, other_module) = (ClassId::from_index(3), ClassId::from_index(4));
        let value = Value::fixnum(7).expect("a fixnum");
        assert_eq!(
            caches.constant(slot, 0, scope, module),
            None,
            "nothing read yet"
        );
        caches.fill_constant(
            slot,
            ConstEntry {
                epoch: 5,
                cref: scope,
                from: module,
                value,
            },
        );
        assert_eq!(caches.constant(slot, 5, scope, module), Some(value));
        assert_eq!(caches.scoped_constant(slot, 5, scope), Some(value));
        assert_eq!(
            caches.constant(slot, 6, scope, module),
            None,
            "a later epoch"
        );
        assert_eq!(caches.constant(slot, 5, other_scope, module), None);
        assert_eq!(caches.constant(slot, 5, scope, other_module), None);
        assert_eq!(caches.scoped_constant(slot, 5, other_scope), None);
        assert_eq!(caches.constant(slot - 1, 5, scope, module), None);
        assert_eq!(caches.filled(), 0, "a constant slot is not a call site");
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

    /// An `Entered` reads the `Iseq` it was made from after the caller's own
    /// `Arc` is gone, since the table keeps one; and `to_arc` is a clone of
    /// that `Arc`, counted as one.
    #[test]
    fn an_entered_iseq_outlives_the_arc_it_was_made_from() {
        let mut caches = CallCaches::new();
        let iseq = Arc::new(Iseq {
            name: "entered".into(),
            ..(*iseq_with(3)).clone()
        });
        let (base, entered) = caches.enter(&iseq);
        assert_eq!(base, caches.base(&iseq), "enter answers base's run");
        drop(iseq);
        assert_eq!(&*entered.name, "entered");
        assert_eq!(entered.call_sites.len(), 3);

        let owned = entered.to_arc();
        assert_eq!(Arc::strong_count(&owned), 2, "the table's and this one");
        assert!(std::ptr::eq(&*owned, &*entered));
        drop(owned);
        assert_eq!(&*entered.name, "entered");
    }

    /// Refilling a site forgets the plan worked out for what it held: the
    /// plan is for that method, and another may answer now.
    #[test]
    fn a_refill_forgets_the_plan() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = crate::shared::symbols::intern("inline_cache_plan");
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        scope
            .classes_mut()
            .define_method(c, name, Value::fixnum(1).unwrap());
        let method = scope.classes_mut().lookup(c, name).unwrap();
        let serial = scope.classes().serial(c);

        let mut caches = CallCaches::new();
        let base = caches.base(&iseq_with(1));
        assert!(matches!(caches.plan(base), Plan::Unknown));
        caches.fill(base, c, serial, method);
        caches.set_plan(base, Plan::General);
        assert!(matches!(caches.plan(base), Plan::General));
        caches.fill(base, c, serial + 1, method);
        assert!(matches!(caches.plan(base), Plan::Unknown), "forgotten");
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

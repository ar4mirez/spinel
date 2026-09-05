//! Classes, modules, method tables, and ancestor chains.
//!
//! # The chain is materialised, not derived
//!
//! Every class and module owns a **run** of the ancestor chain: the modules
//! prepended to it, then the class itself, then the modules included in it. A
//! class's full ancestry is its own run followed by its superclass's, so
//! `ancestors` is a walk and `include` on a superclass is visible to every
//! subclass without anyone being told.
//!
//! The run is maintained by `include`/`prepend` rather than recomputed from a
//! list of mixins, because the order depends on the state of the chain at the
//! moment of each call and cannot be recovered afterwards:
//!
//! ```text
//! module M; end
//! module A; include M; end
//! module B; end
//! class C; include M; include B; include A; end   # [C, A, B, M]
//! ```
//!
//! `A` brings `M`, but `M` is already there, so `A` goes in front of `B` while
//! `M` stays behind it. Replaying `[M, B, A]` against `A`'s final contents puts
//! `M` next to `A` and gets `[C, A, M, B]`. Ruby's answer is the first one.
//!
//! # Where the rules come from
//!
//! [`Classes::include`] and [`Classes::prepend`] are CRuby's `include_modules_at`
//! in flat form: an insertion point that walks forward as modules are spliced in,
//! and a scan that decides what the target can already reach. The four numbers
//! that distinguish the callers are in [`Site`]. Every ordering rule they encode
//! was measured against CRuby rather than read off its source — the table in
//! `crates/spinel-vm/tests/ancestors.txt` is the measurement, and
//! `scripts/ancestors-oracle.rb` is what re-measures it.

use std::collections::HashMap;
use std::fmt;

use crate::heap::{Handle, HandleScope, Payload};
use crate::value::{SymbolId, Value};

/// An index into one heap's [`Classes`] table.
///
/// Not a [`Value`]: the chain, the method tables, and the caches are Rust data,
/// so they index each other with a Rust index. [`Classes::object`] is the
/// crossing point in one direction and [`HandleScope::class_of`] in the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassId(u32);

impl ClassId {
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// One lexical scope, as an index into its heap's cref arena.
///
/// Ruby resolves a constant against the chain of `class`/`module` bodies it was
/// *written* inside, which is not the chain it *runs* inside:
///
/// ```ruby
/// module A
///   X = 1
///   class B
///     def m = X          # A::X, though B.ancestors never reaches A
///   end
/// end
/// ```
///
/// The compiler cannot supply that chain — it knows `B` is nested one level
/// deeper, but `B` is a [`ClassId`] that does not exist until the body runs — so
/// it is built at runtime, exactly as CRuby's cref is.
///
/// An arena index rather than a heap object because a cref is not reachable from
/// Ruby, never outlives the modules it names (which the class table already
/// roots), and is read on the hot path of every constant reference. It is `Copy`,
/// so a frame, a [`Method`], and a `Proc` can each carry one by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CrefId(u32);

impl CrefId {
    /// The scope of a file's top level: `Object`, with nothing outside it.
    /// Seeded by [`HandleScope::bootstrap`] as arena node 0.
    pub const ROOT: CrefId = CrefId(0);

    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// The inverse of [`CrefId::index`], for the slot a `Proc` stores one in.
    #[must_use]
    pub const fn from_index(index: usize) -> CrefId {
        CrefId(index as u32)
    }
}

/// One link of the lexical chain: a module, and the scope enclosing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CrefNode {
    class: ClassId,
    /// `None` only for [`CrefId::ROOT`].
    parent: Option<CrefId>,
}

/// Whether `include` accepts it, and whether it can have a superclass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Class,
    Module,
}

/// A found method, and the class or module that defined it.
///
/// `owner` is where `super` resumes from once [#11] gives it a caller.
///
/// [#11]: https://github.com/ar4mirez/spinel/issues/11
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Method {
    pub owner: ClassId,
    /// The lexical scope the `def` was written in. Carried on the method rather
    /// than derived from `owner`, because a constant referenced in a body
    /// resolves through the enclosing `module`s, which `owner`'s ancestors need
    /// not reach. See [`CrefId`].
    pub cref: CrefId,
    /// The definition. Opaque here — bytecode arrives with [#10].
    ///
    /// [#10]: https://github.com/ar4mirez/spinel/issues/10
    pub body: Value,
}

/// Which end of the class a module is being spliced onto.
///
/// A named pair rather than a `bool`, because every function below that takes it
/// would otherwise take a `prepending: bool` and read wrong at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mixin {
    Include,
    Prepend,
}

impl Mixin {
    const fn verb(self) -> &'static str {
        match self {
            Mixin::Include => "include",
            Mixin::Prepend => "prepend",
        }
    }
}

/// Why a mixin was refused. Both become Ruby exceptions once [#12] can raise one,
/// and both carry the message Ruby uses, because ruby/spec checks message text.
///
/// [#12]: https://github.com/ar4mirez/spinel/issues/12
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixinError {
    /// The argument was a `Class`. Ruby: `TypeError`, "wrong argument type Class
    /// (expected Module)".
    NotAModule,
    /// The target is already in the module's chain, so splicing it in would make
    /// the chain reach itself. Ruby: `ArgumentError`, and the message names the
    /// method that was called — "cyclic include detected" or "cyclic prepend
    /// detected".
    Cyclic(Mixin),
}

impl fmt::Display for MixinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MixinError::NotAModule => f.write_str("wrong argument type Class (expected Module)"),
            MixinError::Cyclic(how) => write!(f, "cyclic {} detected", how.verb()),
        }
    }
}

/// The classes the VM creates before any Ruby runs, in the order it creates
/// them — which is what makes [`Builtin::id`] a cast rather than a lookup.
///
/// `docs/engine.md` names the shells the VM needs to exist before `core/*.rb`
/// can reopen them. `Comparable`, `Enumerable`, and `Numeric` are here because
/// the same sentence asks for "the right ancestry", and without them
/// `Integer.ancestors` is wrong from the first commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Builtin {
    BasicObject,
    Object,
    Module,
    Class,
    Kernel,
    Comparable,
    Enumerable,
    Numeric,
    Symbol,
    String,
    Integer,
    Array,
    Hash,
    Proc,
    Exception,
}

impl Builtin {
    /// In bootstrap order, which is what makes [`Builtin::id`] a cast.
    pub const ALL: [Builtin; 15] = [
        Builtin::BasicObject,
        Builtin::Object,
        Builtin::Module,
        Builtin::Class,
        Builtin::Kernel,
        Builtin::Comparable,
        Builtin::Enumerable,
        Builtin::Numeric,
        Builtin::Symbol,
        Builtin::String,
        Builtin::Integer,
        Builtin::Array,
        Builtin::Hash,
        Builtin::Proc,
        Builtin::Exception,
    ];

    /// Every builtin is defined before anything else, in declaration order.
    pub const fn id(self) -> ClassId {
        ClassId(self as u32)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Builtin::BasicObject => "BasicObject",
            Builtin::Object => "Object",
            Builtin::Module => "Module",
            Builtin::Class => "Class",
            Builtin::Kernel => "Kernel",
            Builtin::Comparable => "Comparable",
            Builtin::Enumerable => "Enumerable",
            Builtin::Numeric => "Numeric",
            Builtin::Symbol => "Symbol",
            Builtin::String => "String",
            Builtin::Integer => "Integer",
            Builtin::Array => "Array",
            Builtin::Hash => "Hash",
            Builtin::Proc => "Proc",
            Builtin::Exception => "Exception",
        }
    }

    /// Look one up by the name Ruby knows it as.
    ///
    /// Not `Object.const_get`: the constant table is [#13]. This is the inverse
    /// of [`Builtin::name`] and nothing more.
    ///
    /// [#13]: https://github.com/ar4mirez/spinel/issues/13
    pub fn from_name(name: &str) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|b| b.name() == name)
    }

    pub const fn kind(self) -> Kind {
        match self {
            Builtin::Kernel | Builtin::Comparable | Builtin::Enumerable => Kind::Module,
            _ => Kind::Class,
        }
    }

    /// `None` for the modules and for `BasicObject`, which is the root.
    const fn superclass(self) -> Option<Builtin> {
        match self {
            Builtin::BasicObject | Builtin::Kernel | Builtin::Comparable | Builtin::Enumerable => {
                None
            }
            Builtin::Object => Some(Builtin::BasicObject),
            Builtin::Module => Some(Builtin::Object),
            Builtin::Class => Some(Builtin::Module),
            Builtin::Integer => Some(Builtin::Numeric),
            _ => Some(Builtin::Object),
        }
    }

    /// The modules each builtin includes, as `core/*.rb` would if it could run
    /// yet. Without them `Integer.ancestors` is wrong from the first commit.
    const fn includes(self) -> &'static [Builtin] {
        match self {
            Builtin::Object => &[Builtin::Kernel],
            Builtin::Numeric | Builtin::Symbol | Builtin::String => &[Builtin::Comparable],
            Builtin::Array | Builtin::Hash => &[Builtin::Enumerable],
            _ => &[],
        }
    }
}

/// One class or module: its object, its chain, its methods, and who mixes it in.
struct Entry {
    /// The heap object Ruby sees. Rooted for as long as the table holds it —
    /// see [`Classes::each_root`].
    object: Value,
    name: Option<Box<str>>,
    kind: Kind,
    superclass: Option<ClassId>,
    /// This class's run of the ancestor chain: prepended modules, then the class
    /// itself at `origin`, then included modules. The superclass's run follows.
    own: Vec<ClassId>,
    /// Index of this class within `own`. CRuby's origin iclass.
    origin: usize,
    methods: HashMap<SymbolId, (Value, CrefId)>,
    /// Every class or module whose `own` holds this one. Flat, not just the
    /// direct includers, which is what lets a later `include` on a module reach
    /// everything that already mixed it in — Ruby 3.0's [Feature #9573] — in one
    /// pass instead of a recursive walk.
    ///
    /// [Feature #9573]: https://bugs.ruby-lang.org/issues/9573
    includers: Vec<ClassId>,
    /// Allocated by [`HandleScope::singleton_class`], never before.
    singleton: Option<ClassId>,
    is_singleton: bool,
    /// This module's own constants. Not the ancestors' — [`Classes::const_get`]
    /// is the walk, and every rule Ruby has about which table wins depends on
    /// this one holding only what was assigned *here*.
    constants: HashMap<SymbolId, Value>,
}

/// One heap's classes and modules.
///
/// Per heap, not global: `docs/engine.md` makes classes shared objects behind
/// the main Ractor's class lock, and [#118] is the slice that has a second
/// Ractor to share them with.
///
/// [#118]: https://github.com/ar4mirez/spinel/issues/118
#[derive(Default)]
pub struct Classes {
    entries: Vec<Entry>,
    /// Bumped by every change that can move a method: a definition, a removal,
    /// an `include`, a `prepend`. Inline caches read it in [#10].
    ///
    // ponytail: one serial for the whole table, where engine.md describes one per
    // class. A shared serial invalidates more than it must — defining a method on
    // any class evicts every cached lookup — which is correct but coarse. Per-class
    // serials need a subclass list and a descendant walk on every definition; the
    // benchmark that would justify writing them arrives with the JIT.
    //
    /// [#10]: https://github.com/ar4mirez/spinel/issues/10
    serial: u64,
    /// `docs/engine.md`'s global method cache, keyed by the receiver's class and
    /// the name. Misses are cached too: a `method_missing` dispatch should not
    /// re-walk the chain on every call.
    ///
    // ponytail: unbounded, where CRuby's global cache was a fixed-size direct-
    // mapped table. It is emptied by every definition, so it cannot outgrow the
    // (class, name) pairs one program actually calls between two definitions;
    // a cap costs an eviction policy, and phase 3 has the profiles to choose one.
    cache: HashMap<(ClassId, SymbolId), Option<Method>>,
    /// Lexical scopes, as an arena of linked-list nodes. See [`CrefId`].
    /// `bootstrap` seeds node 0 with `Object`, which is [`CrefId::ROOT`].
    crefs: Vec<CrefNode>,
}

/// Where a splice starts, and what the target already counts as reaching.
///
/// One struct rather than four near-identical loops. The four constructors are
/// the four callers, and the differences between them are the whole of CRuby's
/// `include_modules_at` argument list.
#[derive(Debug, Clone, Copy)]
struct Site {
    /// Index in `own` the next module is inserted at.
    at: usize,
    /// The entry the insertion point currently sits behind. `None` is the class
    /// head, in front of the whole run, which is where a `prepend` starts. A
    /// module found *before* this point does not move the insertion point —
    /// that is what keeps `include` from walking backwards into the prepends.
    behind: Option<usize>,
    /// The window of `own` the duplicate scan reads, as `scan_from..scan_to`.
    scan_from: usize,
    scan_to: usize,
    /// Whether the scan continues into the superclass's ancestors. False for
    /// `prepend`, which is why prepending a module that a superclass already
    /// includes adds a second copy in front.
    search_super: bool,
}

impl Site {
    /// `include`: behind the class itself, in front of everything it includes.
    /// The scan reads the whole chain, so a module a superclass already has is
    /// not added again.
    fn include(entry: &Entry) -> Site {
        Site {
            at: entry.origin + 1,
            behind: Some(entry.origin),
            scan_from: 0,
            scan_to: entry.own.len(),
            search_super: true,
        }
    }

    /// `prepend`: in front of the whole run. The scan stops at the class itself,
    /// so `include M; prepend M` really does produce two `M`s — CRuby's answer,
    /// and the reason this is a window rather than a whole-chain search.
    fn prepend(entry: &Entry) -> Site {
        Site {
            at: 0,
            behind: None,
            scan_from: 0,
            scan_to: entry.origin,
            search_super: false,
        }
    }

    /// An `include` propagated into someone who already mixed the module in.
    /// The new module lands directly behind it, and only the chain *after* it
    /// counts as already having it.
    fn after(entry: &Entry, anchor: usize) -> Site {
        Site {
            at: anchor + 1,
            behind: Some(anchor),
            scan_from: anchor + 1,
            scan_to: entry.own.len(),
            search_super: true,
        }
    }

    /// A `prepend` propagated the same way. A module prepended to `M` belongs in
    /// front of everything already prepended to `M`, wherever `M` appears — so
    /// the site is the *start* of `M`'s run in the includer, not `M`'s own index,
    /// and the scan window is exactly the prepends already propagated there.
    ///
    /// Anchoring at `M` instead reverses them: two prepends onto a module read
    /// back in one order from the module and the opposite order from anything
    /// that included it.
    fn before(run: std::ops::Range<usize>) -> Site {
        Site {
            at: run.start,
            behind: None,
            scan_from: run.start,
            scan_to: run.end,
            search_super: false,
        }
    }
}

/// Where the duplicate scan found a module, which decides both whether to insert
/// it and whether the insertion point moves.
enum Found {
    /// In the target's own run, at this index.
    Own(usize),
    /// In a superclass's run. The target reaches it, so it is not inserted, but
    /// the insertion point stays where it is: the target's own run is the only
    /// place this class may write.
    Super,
}

impl Classes {
    /// An empty table. [`HandleScope::bootstrap`] is what fills it.
    pub fn new() -> Classes {
        Classes::default()
    }

    /// Bumped by anything that can change what a name resolves to.
    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn kind(&self, id: ClassId) -> Kind {
        self.entry(id).kind
    }

    /// `nil` for an anonymous class; `#<Class:Foo>` for a singleton.
    pub fn name(&self, id: ClassId) -> Option<&str> {
        self.entry(id).name.as_deref()
    }

    pub fn superclass(&self, id: ClassId) -> Option<ClassId> {
        self.entry(id).superclass
    }

    /// The heap object Ruby sees, which is what an instance's header points at.
    pub fn object(&self, id: ClassId) -> Value {
        self.entry(id).object
    }

    /// Whether a singleton class has been allocated for this one yet.
    /// [`HandleScope::singleton_class`] is what allocates it.
    pub fn singleton(&self, id: ClassId) -> Option<ClassId> {
        self.entry(id).singleton
    }

    pub fn is_singleton(&self, id: ClassId) -> bool {
        self.entry(id).is_singleton
    }

    /// `Module#ancestors`: this class's run, then its superclass's, and so on.
    pub fn ancestors(&self, id: ClassId) -> Vec<ClassId> {
        let mut out = Vec::new();
        let mut cursor = Some(id);
        while let Some(c) = cursor {
            out.extend_from_slice(&self.entry(c).own);
            cursor = self.entry(c).superclass;
        }
        out
    }

    /// `Module#include?`: reachable through the chain, and not the class itself.
    pub fn includes(&self, id: ClassId, module: ClassId) -> bool {
        id != module && self.ancestors(id).contains(&module)
    }

    /// Define, redefine, or replace a method on this class or module, in the
    /// top-level lexical scope.
    ///
    /// The right call for a native primitive and for a test: neither resolves a
    /// constant, so neither can tell [`CrefId::ROOT`] from the scope it was
    /// really written in. A `def` compiled from Ruby source must use
    /// [`Classes::define_method_in`] and pass its frame's scope, or a constant
    /// in the body would resolve from `Object` instead of from the enclosing
    /// `module`.
    pub fn define_method(&mut self, id: ClassId, name: SymbolId, body: Value) {
        self.define_method_in(id, name, body, CrefId::ROOT);
    }

    /// Define a method that remembers the lexical scope its `def` appeared in.
    pub fn define_method_in(
        &mut self,
        id: ClassId,
        name: SymbolId,
        body: Value,
        cref: CrefId,
    ) {
        self.entry_mut(id).methods.insert(name, (body, cref));
        self.bump();
    }

    /// `Module#remove_method`: true if there was one to remove.
    pub fn remove_method(&mut self, id: ClassId, name: SymbolId) -> bool {
        let removed = self.entry_mut(id).methods.remove(&name).is_some();
        if removed {
            self.bump();
        }
        removed
    }

    /// Whether this class or module defines the method itself, ancestors aside.
    pub fn method_defined_here(&self, id: ClassId, name: SymbolId) -> bool {
        self.entry(id).methods.contains_key(&name)
    }

    /// Find `name` starting at `id`, through the global method cache.
    ///
    /// Takes `&mut self` because a hit is what a lookup leaves behind; the walk
    /// itself is [`Classes::lookup_uncached`] and borrows nothing mutably. Misses
    /// are cached as `None`, so a name nothing defines costs one chain walk
    /// rather than one per call.
    pub fn lookup(&mut self, id: ClassId, name: SymbolId) -> Option<Method> {
        if let Some(&hit) = self.cache.get(&(id, name)) {
            return hit;
        }
        let found = self.lookup_uncached(id, name);
        self.cache.insert((id, name), found);
        found
    }

    /// The chain walk the cache is in front of. No allocation: the chain is a
    /// run per class and a superclass pointer, so this is two nested loops.
    pub fn lookup_uncached(&self, id: ClassId, name: SymbolId) -> Option<Method> {
        let mut cursor = Some(id);
        while let Some(c) = cursor {
            for &owner in &self.entry(c).own {
                if let Some(&(body, cref)) = self.entry(owner).methods.get(&name) {
                    return Some(Method { owner, body, cref });
                }
            }
            cursor = self.entry(c).superclass;
        }
        None
    }

    // -- constants -------------------------------------------------------

    /// This module's own constant, ignoring every ancestor and every enclosing
    /// scope. The building block the three lookup rules are written from, and
    /// what `class C` itself checks when deciding to reopen or to create.
    #[must_use]
    pub fn const_get_here(&self, id: ClassId, name: SymbolId) -> Option<Value> {
        self.entry(id).constants.get(&name).copied()
    }

    /// Assign, or reassign, a constant on this module.
    ///
    // ponytail: Ruby warns on reassignment ("already initialized constant C").
    // Warning needs somewhere to warn *to*, which is #39's `$stderr`; the write
    // itself is what every spec here checks.
    pub fn const_set(&mut self, id: ClassId, name: SymbolId, value: Value) {
        self.entry_mut(id).constants.insert(name, value);
    }

    /// `A::X`: `A`'s own table, then its ancestors' in order — skipping
    /// `Object`.
    ///
    /// No lexical scope, and `Object` is passed over even though it sits in the
    /// chain. That is Ruby 2.5's change and it is narrower than "no fallback":
    /// `Object` alone is skipped, while `Kernel` and `BasicObject` are searched
    /// like any other ancestor.
    ///
    /// ```ruby
    /// TOP = 1
    /// module Kernel; KC = 2; end
    /// class S; end
    /// S::TOP    # NameError — Object is skipped
    /// S::KC     # 2         — Kernel is not
    /// Object::TOP  # 1      — unless Object is the receiver
    /// ```
    #[must_use]
    pub fn const_get_qualified(&self, id: ClassId, name: SymbolId) -> Option<Value> {
        let object = Builtin::Object.id();
        let skip_object = id != object;
        self.ancestors(id)
            .into_iter()
            .filter(|&c| !(skip_object && c == object))
            .find_map(|c| self.const_get_here(c, name))
    }

    /// A bare `X`, resolved from the scope it was written in.
    ///
    /// Ruby's order, which is documented nowhere and reads wrong from
    /// `variable.c`, so `tests/constants.txt` measures every step of it:
    ///
    /// 1. each module in the cref chain, innermost first, **own table only**;
    /// 2. the ancestors of the innermost cref, in order;
    /// 3. `Object`, if step 2 did not already reach it.
    ///
    /// Step 3 fires for a module body and not for a class body, because a class
    /// reaches `Object` through its superclass chain and a module does not.
    #[must_use]
    pub fn const_get(&self, cref: CrefId, name: SymbolId) -> Option<Value> {
        let mut scope = Some(cref);
        while let Some(c) = scope {
            let node = self.cref(c);
            if let Some(value) = self.const_get_here(node.class, name) {
                return Some(value);
            }
            scope = node.parent;
        }

        // Step 2 searches the whole chain including `Object` — a class body
        // does reach a top-level constant through its superclass — which is why
        // this is not `const_get_qualified`, whose skip is `A::X`'s rule alone.
        let innermost = self.cref(cref).class;
        let object = Builtin::Object.id();
        let ancestors = self.ancestors(innermost);
        if let Some(value) = ancestors.iter().find_map(|&c| self.const_get_here(c, name)) {
            return Some(value);
        }

        // Step 3. A class already reached `Object` above; a module did not.
        if ancestors.contains(&object) {
            return None;
        }
        self.const_get_here(object, name)
    }

    // -- lexical scopes --------------------------------------------------

    /// The module a scope names. `Object` for [`CrefId::ROOT`].
    #[must_use]
    pub fn cref_class(&self, cref: CrefId) -> ClassId {
        self.cref(cref).class
    }

    /// The scope enclosing this one; `None` at the top level.
    #[must_use]
    pub fn cref_parent(&self, cref: CrefId) -> Option<CrefId> {
        self.cref(cref).parent
    }

    /// Open a scope for `class` nested inside `outer`.
    ///
    /// Nodes are never freed: a scope lives as long as the bodies compiled
    /// inside it, which is as long as the heap. One `u64` per `class` keyword
    /// executed, and `class` inside a loop reopens rather than re-pushing.
    pub fn push_cref(&mut self, outer: CrefId, class: ClassId) -> CrefId {
        let id = CrefId(self.crefs.len() as u32);
        self.crefs.push(CrefNode {
            class,
            parent: Some(outer),
        });
        id
    }

    /// Install [`CrefId::ROOT`]. Called once, by `bootstrap`, before any Ruby
    /// runs; `const_get` indexes the arena unconditionally and a heap with no
    /// node 0 would panic rather than answer.
    pub(crate) fn seed_root_cref(&mut self) {
        assert!(self.crefs.is_empty(), "the root scope is already seeded");
        self.crefs.push(CrefNode {
            class: Builtin::Object.id(),
            parent: None,
        });
    }

    fn cref(&self, id: CrefId) -> &CrefNode {
        &self.crefs[id.index()]
    }

    /// How many lookups the cache is currently answering. For tests and, in
    /// phase 2, `RubyVM.stat`.
    pub fn cached_lookups(&self) -> usize {
        self.cache.len()
    }

    /// `Module#include`. Splices the module's whole run in behind the class.
    ///
    /// One module: Ruby's `Module#include(*modules)` is this applied right to
    /// left, which is why `include A, B` leaves `A` closer than `B`.
    pub fn include(&mut self, target: ClassId, module: ClassId) -> Result<(), MixinError> {
        self.mixin(target, module, Mixin::Include)
    }

    /// `Module#prepend`. Splices it in front of the class, where it wins lookup.
    pub fn prepend(&mut self, target: ClassId, module: ClassId) -> Result<(), MixinError> {
        self.mixin(target, module, Mixin::Prepend)
    }

    fn mixin(&mut self, target: ClassId, module: ClassId, how: Mixin) -> Result<(), MixinError> {
        if self.entry(module).kind == Kind::Class {
            return Err(MixinError::NotAModule);
        }
        // The module's run already holds everything it reaches, so one `contains`
        // catches both `M.include M` and a mutual include two modules apart.
        if self.entry(module).own.contains(&target) {
            return Err(MixinError::Cyclic(how));
        }

        let site = match how {
            Mixin::Include => Site::include(self.entry(target)),
            Mixin::Prepend => Site::prepend(self.entry(target)),
        };
        self.splice(target, site, module);

        // Everything that already reaches `target` gets the same modules at its
        // own copy of it. `includers` is flat, so a module three levels down is
        // patched here and not by recursing — and patching each includer at its
        // own anchor is what keeps the new module next to the one that brought
        // it rather than next to the class.
        for includer in self.entry(target).includers.clone() {
            let Some(anchor) = self.entry(includer).own.iter().position(|&m| m == target) else {
                continue;
            };
            let site = match how {
                Mixin::Include => Site::after(self.entry(includer), anchor),
                Mixin::Prepend => Site::before(self.run_of(includer, target, anchor)),
            };
            self.splice(includer, site, module);
        }

        self.bump();
        Ok(())
    }

    /// Splice `source`'s run into `target`'s at `site`, skipping every module
    /// the target already reaches.
    ///
    /// The insertion point walks forward past each module it inserts, and *also*
    /// past each one it finds already in place — which is the rule that puts a
    /// shared module behind the modules that were there before it rather than
    /// dragging it forward.
    fn splice(&mut self, target: ClassId, mut site: Site, source: ClassId) {
        // `source`'s run can change under us only through `target`, and a mixin
        // that would let that happen was rejected as cyclic above.
        let chain = self.entry(source).own.clone();
        for module in chain {
            match self.find(target, module, &site) {
                Some(Found::Own(q)) if site.behind.is_none_or(|behind| q >= behind) => {
                    site.behind = Some(q);
                    site.at = q + 1;
                }
                Some(Found::Own(_) | Found::Super) => {}
                None => {
                    self.insert(target, site.at, module);
                    site.behind = Some(site.at);
                    if site.at <= site.scan_to {
                        site.scan_to += 1;
                    }
                    site.at += 1;
                }
            }
        }
    }

    /// Where `module`'s run starts inside `target`, given where `module` itself
    /// sits: its own index, walked back over the entries that were propagated
    /// there from `module`'s own prepends.
    ///
    /// This is the flat form of CRuby's iclass head, which sits in front of the
    /// prepends it brought while `module` itself sits behind them.
    fn run_of(&self, target: ClassId, module: ClassId, anchor: usize) -> std::ops::Range<usize> {
        let entry = self.entry(module);
        let prepends = &entry.own[..entry.origin];
        let own = &self.entry(target).own;
        let mut start = anchor;
        while start > 0 && prepends.contains(&own[start - 1]) {
            start -= 1;
        }
        start..anchor
    }

    fn find(&self, target: ClassId, module: ClassId, site: &Site) -> Option<Found> {
        let entry = self.entry(target);
        for (q, &m) in entry
            .own
            .iter()
            .enumerate()
            .take(site.scan_to)
            .skip(site.scan_from)
        {
            if m == module {
                return Some(Found::Own(q));
            }
        }
        if site.search_super {
            let mut cursor = entry.superclass;
            while let Some(c) = cursor {
                if self.entry(c).own.contains(&module) {
                    return Some(Found::Super);
                }
                cursor = self.entry(c).superclass;
            }
        }
        None
    }

    /// Put `module` at `at` in `target`'s run, moving the class itself along if
    /// the module went in front of it, and recording the target as an includer.
    fn insert(&mut self, target: ClassId, at: usize, module: ClassId) {
        let entry = self.entry_mut(target);
        entry.own.insert(at, module);
        if at <= entry.origin {
            entry.origin += 1;
        }
        let includers = &mut self.entry_mut(module).includers;
        if !includers.contains(&target) {
            includers.push(target);
        }
    }

    /// Every method lookup answered before this point may now be wrong.
    ///
    /// The cache is cleared rather than stamped and left, because an entry can
    /// name a body that `remove_method` just dropped — the only reference the
    /// collector would still be tracing.
    fn bump(&mut self) {
        self.serial += 1;
        self.cache.clear();
    }

    fn entry(&self, id: ClassId) -> &Entry {
        &self.entries[id.0 as usize]
    }

    fn entry_mut(&mut self, id: ClassId) -> &mut Entry {
        &mut self.entries[id.0 as usize]
    }

    /// Register an already-allocated object. Private: every caller has to go
    /// through [`HandleScope`], which is what allocated the object.
    fn define(
        &mut self,
        object: Value,
        name: Option<&str>,
        kind: Kind,
        superclass: Option<ClassId>,
        is_singleton: bool,
    ) -> ClassId {
        let id = ClassId(u32::try_from(self.entries.len()).expect("a heap holds under 4B classes"));
        self.entries.push(Entry {
            object,
            name: name.map(Box::from),
            kind,
            superclass,
            own: vec![id],
            origin: 0,
            methods: HashMap::new(),
            includers: Vec::new(),
            singleton: None,
            is_singleton,
            constants: HashMap::new(),
        });
        self.bump();
        id
    }

    /// Every `Value` the table keeps alive: each class object, and every method
    /// body. The collector calls this; see `Heap::mark`.
    ///
    // ponytail: linear in the number of methods defined, on every collection,
    // where the handle stack is linear in what Rust is holding right now. With a
    // full `core/*.rb` that is thousands of entries per GC and still small beside
    // the trace it precedes. It stops being a walk at all when #151's shapes let
    // a method table be a heap object like anything else.
    pub(crate) fn each_root(&self, mut f: impl FnMut(Value)) {
        for entry in &self.entries {
            f(entry.object);
            for &(body, _) in entry.methods.values() {
                f(body);
            }
            for &value in entry.constants.values() {
                f(value);
            }
        }
    }
}

impl fmt::Debug for Classes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Classes")
            .field("classes", &self.entries.len())
            .field("serial", &self.serial)
            .field("cached", &self.cache.len())
            .finish()
    }
}

/// A class object's slots. One for now: the id, so that an instance's header
/// class pointer can be resolved back to a table entry.
///
// ponytail: an ivar in a fixed slot, because shapes are not here yet. When #151
// lands the shape tree, this becomes an ordinary hidden instance variable and
// `class_of` reads it through the shape.
const SLOT_ID: usize = 0;
const CLASS_SLOTS: u32 = 1;

impl<'h> HandleScope<'h> {
    /// Create the classes `docs/engine.md`'s boot order step 1 asks for.
    ///
    /// Called once per heap, before anything else. The order matches [`Builtin`],
    /// so `Builtin::Object.id()` is a cast.
    ///
    /// # Panics
    ///
    /// If the heap already has classes.
    pub fn bootstrap(&mut self) {
        assert!(
            self.classes().is_empty(),
            "the heap is already bootstrapped"
        );

        for builtin in Builtin::ALL {
            let superclass = builtin.superclass().map(Builtin::id);
            let id = self.define(Some(builtin.name()), builtin.kind(), superclass, false);
            assert_eq!(
                id,
                builtin.id(),
                "{} is out of bootstrap order",
                builtin.name()
            );
        }
        for builtin in Builtin::ALL {
            for &module in builtin.includes() {
                self.classes_mut()
                    .include(builtin.id(), module.id())
                    .expect("bootstrap mixes in modules, and its chains are acyclic");
            }
        }

        // Until `Class` and `Module` existed there was nothing to point a class
        // object at, so the first four were allocated without one. Nothing has run
        // in between, so patching them now is the same as never having missed it.
        for index in 0..self.classes().len() {
            let id = ClassId(index as u32);
            let meta = match self.classes().kind(id) {
                Kind::Class => Builtin::Class,
                Kind::Module => Builtin::Module,
            };
            let mut scope = self.nested();
            let object = scope.classes().object(id);
            let handle = scope.root(object);
            let meta = scope.classes().object(meta.id());
            scope.set_class(handle, meta);
        }
        // The top level's lexical scope, and the one every other scope is
        // eventually nested inside. Seeded here so `CrefId::ROOT` is a constant
        // that `Frame::new` can name without a heap.
        self.classes_mut().seed_root_cref();

        // Every bootstrap class is reachable by name from the top level, which
        // means a constant on `Object`. Ruby's own `Object.const_get(:String)`
        // resolves through exactly this table.
        for builtin in Builtin::ALL {
            let name = crate::shared::symbols::intern(builtin.name());
            let object = self.classes().object(builtin.id());
            self.classes_mut()
                .const_set(Builtin::Object.id(), name, object);
        }

        // The handful of methods that are dispatch rather than Ruby. A heap
        // with classes is one where a `Proc` can be called; everything else
        // waits for `core/*.rb`.
        crate::interp::install_primitives(self);
    }

    /// A new class. `superclass` is `None` only for `BasicObject`.
    pub fn define_class(&mut self, name: Option<&str>, superclass: Option<ClassId>) -> ClassId {
        self.define(name, Kind::Class, superclass, false)
    }

    /// A new module. Modules have no superclass; their run is their whole chain.
    pub fn define_module(&mut self, name: Option<&str>) -> ClassId {
        self.define(name, Kind::Module, None, false)
    }

    fn define(
        &mut self,
        name: Option<&str>,
        kind: Kind,
        superclass: Option<ClassId>,
        is_singleton: bool,
    ) -> ClassId {
        // Nested, because the table is what roots a class for the rest of the
        // heap's life. Handing the caller's scope a second, permanent root per
        // class would grow the root stack by every class a program ever defines.
        let mut scope = self.nested();

        // A class object's own class. `Class` and `Module` do not exist yet while
        // they are themselves being bootstrapped, so the first four classes are
        // allocated without one and patched at the end of `bootstrap`.
        let meta = match kind {
            Kind::Class => Builtin::Class,
            Kind::Module => Builtin::Module,
        };
        let meta = (scope.classes().len() > meta.id().0 as usize)
            .then(|| scope.classes().object(meta.id()))
            .map(|object| scope.root(object));

        let handle = scope.alloc(meta, Payload::Slots, CLASS_SLOTS);
        let object = scope.get(handle);
        // No allocation between here and the table entry, so the object cannot be
        // collected before the table is rooting it.
        let id = scope
            .classes_mut()
            .define(object, name, kind, superclass, is_singleton);
        let slot = Value::fixnum(i64::from(id.0)).expect("a class id fits in a fixnum");
        scope.set_slot(handle, SLOT_ID, slot);
        id
    }

    /// The singleton class of a class or module, allocating it on first ask.
    ///
    /// A class's singleton inherits from its superclass's singleton, so a class
    /// method is inherited the same way an instance method is. `BasicObject`'s
    /// singleton inherits from `Class`, which is the twist that makes `Class`,
    /// `Module`, and `Object` reachable from every metaclass. A module has no
    /// superclass, so its singleton inherits from `Module` directly.
    pub fn singleton_class(&mut self, id: ClassId) -> ClassId {
        if let Some(singleton) = self.classes().singleton(id) {
            return singleton;
        }
        let superclass = match self.classes().kind(id) {
            // Recurses up the superclass chain, allocating the singletons it
            // passes. They are the ancestors of the one being asked for, so they
            // are not speculative: nothing here is allocated that the answer does
            // not contain.
            Kind::Class => Some(match self.classes().superclass(id) {
                Some(superclass) => self.singleton_class(superclass),
                None => Builtin::Class.id(),
            }),
            Kind::Module => Some(Builtin::Module.id()),
        };
        let name = self
            .classes()
            .name(id)
            .map(|name| format!("#<Class:{name}>"));
        let singleton = self.define(name.as_deref(), Kind::Class, superclass, true);
        self.classes_mut().entry_mut(id).singleton = Some(singleton);
        // The same header write `singleton_class_of` makes for an ordinary
        // object, and for the same reason: dispatch reads the header, so a
        // metaclass the table knows about and the header does not is a
        // metaclass no call can ever reach. `C.m` would look in `Class`.
        let mut scope = self.nested();
        let object = scope.classes().object(id);
        let handle = scope.root(object);
        let meta = scope.classes().object(singleton);
        scope.set_class(handle, meta);
        singleton
    }

    /// The singleton class of an ordinary object, allocating it on first ask.
    ///
    /// Ruby's model exactly: the object's class *becomes* the singleton, and the
    /// singleton inherits from the class it replaced. So the header write is the
    /// whole of it, and asking twice returns the same class because the second
    /// ask finds a singleton already there.
    ///
    /// # Panics
    ///
    /// If the object has no class — that is, if it was allocated before
    /// [`HandleScope::bootstrap`].
    pub fn singleton_class_of(&mut self, handle: Handle<'h>) -> ClassId {
        let class = self
            .class_of(handle)
            .expect("an object has a class once the heap is bootstrapped");
        if self.classes().is_singleton(class) {
            return class;
        }
        // Anonymous: naming it needs `inspect` on the object it belongs to.
        let singleton = self.define(None, Kind::Class, Some(class), true);
        let object = self.classes().object(singleton);
        self.set_class(handle, object);
        singleton
    }

    /// The class of a heap object, as a table entry. `None` for an immediate,
    /// which has no header to read.
    /// Takes `&mut self` because reading the class object's slot means rooting
    /// it first, and that is a push onto the root stack. Reaching past the
    /// handle discipline to avoid it would be the one unsafe block in this file.
    /// The table entry a value *is*, as opposed to the one it is an instance of.
    ///
    /// `Some` only for a class or module object. `class_of` answers "what is
    /// this an instance of"; this answers "is this a class, and which one" —
    /// the question `A::X`, `class A::B`, and `def self.foo` all ask.
    ///
    /// A class object is exactly one `Slots` cell holding its own id, so the
    /// check is a shape test plus a round-trip through [`Classes::object`]. The
    /// round-trip is what rules out an ordinary one-slot object whose slot
    /// happens to hold a small integer.
    pub fn class_id_of(&mut self, handle: Handle<'h>) -> Option<ClassId> {
        if self.payload(handle) != Payload::Slots || self.len(handle) != CLASS_SLOTS {
            return None;
        }
        let id = self.slot(handle, SLOT_ID).as_fixnum()?;
        let id = ClassId(u32::try_from(id).ok()?);
        let value = self.get(handle);
        (id.index() < self.classes().len() && self.classes().object(id) == value).then_some(id)
    }

    pub fn class_of(&mut self, handle: Handle<'h>) -> Option<ClassId> {
        let class = self.class(handle)?;
        let mut inner = self.nested();
        let class = inner.root(class);
        // `expect` rather than `?`: `None` here would mean "this object has no
        // class", and an object whose header points at something that is not a
        // class is a bug in the caller, not an object without one.
        let id = inner
            .slot(class, SLOT_ID)
            .as_fixnum()
            .expect("a class object carries its id in slot 0");
        Some(ClassId(
            u32::try_from(id).expect("a class id was written from a `ClassId`"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heap::Heap;

    /// Ancestors by name, which is how the CRuby table in `tests/ancestors.txt`
    /// reads and how a failure here is legible.
    fn ancestors(classes: &Classes, id: ClassId) -> Vec<&str> {
        classes
            .ancestors(id)
            .into_iter()
            .map(|a| classes.name(a).unwrap_or("?"))
            .collect()
    }

    fn booted() -> Heap {
        let mut heap = Heap::new();
        heap.scope().bootstrap();
        heap
    }

    // T1 — engine.md's boot order step 1: shells "with the right ancestry".
    #[test]
    fn the_bootstrap_hierarchy_is_rubys() {
        let heap = booted();
        let classes = heap.classes();

        for builtin in Builtin::ALL {
            assert_eq!(classes.name(builtin.id()), Some(builtin.name()));
            assert_eq!(classes.kind(builtin.id()), builtin.kind());
        }

        assert_eq!(classes.superclass(Builtin::BasicObject.id()), None);
        assert_eq!(
            ancestors(classes, Builtin::BasicObject.id()),
            ["BasicObject"]
        );
        assert_eq!(
            ancestors(classes, Builtin::Object.id()),
            ["Object", "Kernel", "BasicObject"]
        );
        assert_eq!(
            ancestors(classes, Builtin::Class.id()),
            ["Class", "Module", "Object", "Kernel", "BasicObject"]
        );
        // The one that is wrong without `Numeric` and `Comparable`, which is why
        // both are bootstrapped even though engine.md's list stops short of them.
        assert_eq!(
            ancestors(classes, Builtin::Integer.id()),
            [
                "Integer",
                "Numeric",
                "Comparable",
                "Object",
                "Kernel",
                "BasicObject"
            ]
        );
        assert_eq!(
            ancestors(classes, Builtin::Array.id()),
            ["Array", "Enumerable", "Object", "Kernel", "BasicObject"]
        );
        // A module's chain is its own run: no superclass, and no `Object`.
        assert_eq!(ancestors(classes, Builtin::Kernel.id()), ["Kernel"]);
    }

    // T2 — the crossing point between a header's class pointer and the table.
    #[test]
    fn an_instance_resolves_back_to_its_class() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let point = scope.define_class(Some("Point"), Some(Builtin::Object.id()));

        let class_object = scope.classes().object(point);
        let class_handle = scope.root(class_object);
        let instance = scope.alloc(Some(class_handle), Payload::Slots, 2);

        assert_eq!(scope.class_of(instance), Some(point));
        assert_eq!(scope.class_of(class_handle), Some(Builtin::Class.id()));

        // And a module object is an instance of `Module`, not of `Class`.
        let unit = scope.define_module(Some("Unit"));
        let unit_object = scope.classes().object(unit);
        let unit_handle = scope.root(unit_object);
        assert_eq!(scope.class_of(unit_handle), Some(Builtin::Module.id()));
    }

    // T3 — the issue's second box.
    #[test]
    fn singleton_classes_are_allocated_on_the_first_ask() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let a = scope.define_class(Some("A"), Some(Builtin::Object.id()));
        let b = scope.define_class(Some("B"), Some(a));

        let before = scope.classes().len();
        assert_eq!(scope.classes().singleton(b), None, "nothing yet");
        assert_eq!(scope.classes().len(), before, "asking did not allocate one");

        let singleton = scope.singleton_class(b);
        assert_eq!(scope.classes().singleton(b), Some(singleton));
        assert!(scope.classes().is_singleton(singleton));
        // `B`'s, `A`'s, `Object`'s, and `BasicObject`'s: exactly the four the
        // answer contains, and not one speculative metaclass more.
        assert_eq!(scope.classes().len(), before + 4);

        let again = scope.singleton_class(b);
        assert_eq!(again, singleton, "and the second ask allocates nothing");
        assert_eq!(scope.classes().len(), before + 4);
    }

    // T4 — an object's class *becoming* its singleton, which is Ruby's mechanism
    // rather than a side table.
    #[test]
    fn an_objects_singleton_replaces_the_class_in_its_header() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let point = scope.define_class(Some("Point"), Some(Builtin::Object.id()));
        let class_object = scope.classes().object(point);
        let class_handle = scope.root(class_object);

        let one = scope.alloc(Some(class_handle), Payload::Slots, 1);
        let two = scope.alloc(Some(class_handle), Payload::Slots, 1);

        let singleton = scope.singleton_class_of(one);
        assert_eq!(scope.class_of(one), Some(singleton));
        assert_eq!(scope.classes().superclass(singleton), Some(point));
        assert_eq!(
            ancestors(scope.classes(), singleton),
            ["?", "Point", "Object", "Kernel", "BasicObject"],
            "the singleton is anonymous, and inherits everything Point has"
        );
        assert_eq!(scope.singleton_class_of(one), singleton, "asked twice");
        // The sibling is untouched: a singleton is one object's, not the class's.
        assert_eq!(scope.class_of(two), Some(point));
    }

    // T5
    #[test]
    fn lookup_walks_the_chain_and_a_prepended_module_wins() {
        let mut heap = booted();
        let mut scope = heap.scope();
        // Interned rather than fabricated: this asserts the name is *absent*
        // from a chain that reaches `Kernel`, where `bootstrap` defines the
        // primitives, so a raw id could collide with one of their names.
        let name = crate::shared::symbols::intern("chain_test_name");
        let base = scope.define_class(Some("Base"), Some(Builtin::Object.id()));
        let sub = scope.define_class(Some("Sub"), Some(base));
        let included = scope.define_module(Some("Included"));
        let prepended = scope.define_module(Some("Prepended"));

        let classes = scope.classes_mut();
        classes.include(sub, included).unwrap();
        assert_eq!(classes.lookup(sub, name), None, "nothing defines it yet");

        // Furthest first, so each definition has to win over the one before it.
        for (owner, body) in [(base, 1), (included, 2), (sub, 3), (prepended, 4)] {
            classes.define_method(owner, name, Value::fixnum(body).unwrap());
            if owner == prepended {
                classes.prepend(sub, prepended).unwrap();
            }
            let found = classes.lookup(sub, name).expect("defined");
            assert_eq!(found.owner, owner, "{:?}", ancestors(classes, sub));
            assert_eq!(found.body, Value::fixnum(body).unwrap());
        }

        // Removing the winner falls back through the chain, one step at a time.
        for (removed, next) in [(prepended, sub), (sub, included), (included, base)] {
            assert!(classes.remove_method(removed, name));
            assert_eq!(
                classes.lookup(sub, name).expect("still defined").owner,
                next
            );
        }
        assert!(classes.remove_method(base, name));
        assert_eq!(classes.lookup(sub, name), None);
        assert!(!classes.remove_method(base, name), "nothing left to remove");
    }

    // T6 — the cache is in front of the walk, and every change that can move a
    // method empties it. ruby/spec spells this one "clears any caches".
    #[test]
    fn the_method_cache_is_emptied_by_anything_that_can_move_a_method() {
        let mut heap = booted();
        let mut scope = heap.scope();
        // Interned rather than fabricated: `bootstrap` defines the primitives
        // on `Kernel`, which is in `C`'s ancestry, so a raw `SymbolId` can
        // collide with one of their names and make a "missing" method found.
        let name = crate::shared::symbols::intern("cache_test_defined");
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        let m1 = scope.define_module(Some("M1"));
        let m2 = scope.define_module(Some("M2"));
        let classes = scope.classes_mut();
        classes.define_method(m1, name, Value::fixnum(1).unwrap());
        classes.define_method(m2, name, Value::fixnum(2).unwrap());
        classes.include(c, m1).unwrap();

        assert_eq!(classes.cached_lookups(), 0);
        assert_eq!(classes.lookup(c, name).unwrap().owner, m1);
        assert_eq!(classes.cached_lookups(), 1, "the answer was kept");
        assert_eq!(classes.lookup(c, name).unwrap().owner, m1, "and reused");

        // A module included after the call has to be seen by the next one.
        let serial = classes.serial();
        classes.include(c, m2).unwrap();
        assert!(classes.serial() > serial);
        assert_eq!(classes.cached_lookups(), 0, "the cache went with it");
        assert_eq!(classes.lookup(c, name).unwrap().owner, m2);

        // Misses are cached too, and invalidated the same way.
        let missing = crate::shared::symbols::intern("cache_test_missing");
        assert_eq!(classes.lookup(c, missing), None);
        assert_eq!(classes.cached_lookups(), 2);
        classes.define_method(m2, missing, Value::fixnum(3).unwrap());
        assert_eq!(classes.lookup(c, missing).unwrap().owner, m2);
    }

    // T7
    #[test]
    fn a_class_argument_and_a_cycle_are_both_refused() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        let a = scope.define_module(Some("A"));
        let b = scope.define_module(Some("B"));
        let classes = scope.classes_mut();

        assert_eq!(classes.include(a, c), Err(MixinError::NotAModule));
        assert_eq!(classes.prepend(a, c), Err(MixinError::NotAModule));
        assert_eq!(
            classes.include(a, a),
            Err(MixinError::Cyclic(Mixin::Include))
        );
        assert_eq!(
            classes.prepend(a, a),
            Err(MixinError::Cyclic(Mixin::Prepend))
        );

        // ruby/spec spells these "detects cyclic includes" and "detects cyclic
        // prepends", and Ruby's messages name the method that was called.
        assert_eq!(
            MixinError::Cyclic(Mixin::Include).to_string(),
            "cyclic include detected"
        );
        assert_eq!(
            MixinError::Cyclic(Mixin::Prepend).to_string(),
            "cyclic prepend detected"
        );
        assert_eq!(
            MixinError::NotAModule.to_string(),
            "wrong argument type Class (expected Module)"
        );

        // Mutual inclusion two modules apart: `B` reaches `A`, so `A` including
        // `B` would make the chain reach itself.
        classes.include(b, a).unwrap();
        assert_eq!(
            classes.include(a, b),
            Err(MixinError::Cyclic(Mixin::Include))
        );
        assert_eq!(
            classes.prepend(a, b),
            Err(MixinError::Cyclic(Mixin::Prepend))
        );

        // A refusal changes nothing.
        assert_eq!(ancestors(classes, a), ["A"]);
        assert_eq!(ancestors(classes, b), ["B", "A"]);

        // Three apart, where the cycle exists only because the middle module's
        // later `include` was propagated back into the first. One `contains` on
        // a materialised chain catches it; a check that only compared the two
        // arguments would not.
        let d = scope.define_module(Some("D"));
        let e = scope.define_module(Some("E"));
        let f = scope.define_module(Some("F"));
        let classes = scope.classes_mut();
        classes.include(d, e).unwrap();
        classes.include(e, f).unwrap();
        assert_eq!(ancestors(classes, d), ["D", "E", "F"]);
        assert_eq!(
            classes.include(f, d),
            Err(MixinError::Cyclic(Mixin::Include))
        );
    }

    // T8 — the class table is a root source, which is the part of #8 that #7's
    // collector could not have been written without.
    #[test]
    fn the_collector_traces_class_objects_and_method_bodies() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = SymbolId(3);
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));

        {
            // The body is rooted by nothing but the method table once this drops.
            let mut inner = scope.nested();
            let body = inner.alloc(None, Payload::Slots, 1);
            inner.set_slot(body, 0, Value::fixnum(42).unwrap());
            let body = inner.get(body);
            inner.classes_mut().define_method(c, name, body);
        }
        scope.collect();

        // Every class object survived, with no handle rooting any of them.
        assert!(
            scope.stats().live_objects > Builtin::ALL.len(),
            "{:?}",
            scope.stats()
        );
        let class_object = scope.classes().object(c);
        let class_handle = scope.root(class_object);
        assert_eq!(scope.class_of(class_handle), Some(Builtin::Class.id()));

        // And so did the body, which is only reachable through the method table.
        let body = scope.classes_mut().lookup(c, name).expect("survived").body;
        let body = scope.root(body);
        assert_eq!(scope.slot(body, 0), Value::fixnum(42).unwrap());
    }

    #[test]
    fn the_collector_traces_constants() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let name = SymbolId(4);
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));

        {
            // Rooted by the constant table and nothing else once this drops. A
            // `Classes::each_root` that walked methods but not constants would
            // free this and leave `C::K` pointing at a reused cell.
            let mut inner = scope.nested();
            let value = inner.alloc(None, Payload::Slots, 1);
            inner.set_slot(value, 0, Value::fixnum(7).unwrap());
            let value = inner.get(value);
            inner.classes_mut().const_set(c, name, value);
        }
        scope.collect();

        let found = scope
            .classes()
            .const_get_here(c, name)
            .expect("the constant survived");
        let handle = scope.root(found);
        assert_eq!(scope.slot(handle, 0), Value::fixnum(7).unwrap());
    }

    #[test]
    fn a_scope_chain_reads_outward_and_object_is_its_end() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let outer = scope.define_module(Some("Outer"));
        let inner = scope.define_class(Some("Inner"), Some(Builtin::Object.id()));

        let outer_scope = scope.classes_mut().push_cref(CrefId::ROOT, outer);
        let inner_scope = scope.classes_mut().push_cref(outer_scope, inner);

        let name = SymbolId(5);
        scope.classes_mut().const_set(outer, name, Value::fixnum(1).unwrap());
        assert_eq!(
            scope.classes().const_get(inner_scope, name),
            Some(Value::fixnum(1).unwrap()),
            "a lexically enclosing module is reached though `Inner`'s ancestors never touch it"
        );

        // The innermost scope's own table wins over the enclosing one.
        scope.classes_mut().const_set(inner, name, Value::fixnum(2).unwrap());
        assert_eq!(
            scope.classes().const_get(inner_scope, name),
            Some(Value::fixnum(2).unwrap())
        );

        // `Object` is the end of every chain, and is reached from a module body
        // even though a module's ancestors do not include it.
        let top = SymbolId(6);
        scope
            .classes_mut()
            .const_set(Builtin::Object.id(), top, Value::fixnum(3).unwrap());
        assert_eq!(
            scope.classes().const_get(outer_scope, top),
            Some(Value::fixnum(3).unwrap())
        );
        // A qualified lookup gets no such fallback: `Outer::TOP` is a NameError
        // in Ruby, which is what removing it in 2.5 means.
        assert_eq!(scope.classes().const_get_qualified(outer, top), None);
    }

    // T9
    #[test]
    fn the_serial_moves_for_every_change_that_can_move_a_method() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        let m = scope.define_module(Some("M"));
        let name = SymbolId(4);
        let classes = scope.classes_mut();

        let mut serial = classes.serial();
        let mut moved = |classes: &Classes, what: &str| {
            assert!(classes.serial() > serial, "{what} left the serial alone");
            serial = classes.serial();
        };
        classes.define_method(m, name, Value::fixnum(1).unwrap());
        moved(classes, "define_method");
        classes.include(c, m).unwrap();
        moved(classes, "include");
        classes.prepend(c, m).unwrap();
        moved(classes, "prepend");
        classes.remove_method(m, name);
        moved(classes, "remove_method");

        // And not for a removal that removed nothing.
        let before = classes.serial();
        assert!(!classes.remove_method(m, name));
        assert_eq!(classes.serial(), before);
    }

    #[test]
    fn include_reports_what_the_chain_reaches() {
        let mut heap = booted();
        let mut scope = heap.scope();
        let c = scope.define_class(Some("C"), Some(Builtin::Object.id()));
        let m = scope.define_module(Some("M"));
        let classes = scope.classes_mut();
        classes.include(c, m).unwrap();

        assert!(classes.includes(c, m));
        assert!(classes.includes(c, Builtin::Kernel.id()), "through Object");
        assert!(!classes.includes(c, c), "a class does not include itself");
        assert!(!classes.includes(m, c));

        // `method_defined_here` is the `false` argument to Ruby's
        // `Module#instance_methods`: what this one defines, ancestors aside.
        let name = SymbolId(1);
        classes.define_method(m, name, Value::fixnum(1).unwrap());
        assert!(classes.method_defined_here(m, name));
        assert!(!classes.method_defined_here(c, name), "C inherits it");
        assert!(classes.lookup(c, name).is_some(), "but still finds it");
    }

    #[test]
    #[should_panic(expected = "already bootstrapped")]
    fn bootstrapping_twice_is_a_bug() {
        let mut heap = booted();
        heap.scope().bootstrap();
    }
}

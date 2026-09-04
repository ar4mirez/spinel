# PRD 0008 — Bootstrap classes, method tables, and ancestor chains

Tracks [#8](https://github.com/ar4mirez/spinel/issues/8). Milestone: Phase 1: a VM that
runs `language/`. `P0`, `size:L`, `area:engine`.

## Objective

[#7](https://github.com/ar4mirez/spinel/issues/7) gave every object a header with a class
pointer that the collector traces and nothing can fill. This slice is what goes in it: the
class hierarchy Ruby starts with, a method table per class, and the ancestor chain that
decides which of those tables a name is found in.

The chain is the load-bearing part, and it is harder than it looks. `include` and
`prepend` are not "push onto a list": where a module lands depends on what the chain
already reaches at the moment of the call, a module included into a module reaches back
into everything that already mixed it in, and several of the resulting orderings are
surprising enough that reading CRuby's `class.c` and believing the result is the wrong
move. Two of them produce a *duplicate* entry in `ancestors`, which is not what anyone
would design.

So the rules here were measured rather than transcribed.
`crates/spinel-vm/tests/ancestors.txt` is 42 hierarchies and the 52 answers a real Ruby
gives for them; `scripts/ancestors-oracle.rb` is what produced them and what CI re-runs;
`crates/spinel-vm/tests/ancestors.rs` is what holds Spinel to them. The table is the
definition of done, and it is falsifiable in both directions: Ruby can disagree with the
table, and Spinel can disagree with the table, and CI fails on either.

## Non-goals

- **Shapes and instance variables.** [#151](https://github.com/ar4mirez/spinel/issues/151)
  has the shape tree — an issue this slice's triage had to open, because engine.md commits
  to shapes and nothing tracked them. The two reserved header bytes stay reserved; a class object carries its
  table id in a fixed slot until an ordinary hidden ivar can hold it.
- **Method *bodies*.** A method entry is an opaque `Value` here, because bytecode arrives
  with [#10](https://github.com/ar4mirez/spinel/issues/10) and calling one with
  [#11](https://github.com/ar4mirez/spinel/issues/11). What this slice owes them is the
  lookup that finds the entry and the `owner` that `super` resumes from.
- **Constants, and `Module#name` as Ruby computes it.** A class here carries the string it
  was defined with. `Object.const_get`, nesting, `const_missing`, and the anonymous-to-
  named transition are [#13](https://github.com/ar4mirez/spinel/issues/13).
- **Dispatch on immediates.** `1.class` needs `NilClass`, `TrueClass`, `FalseClass`, and
  `Float`, and those are `core/*.rb`'s
  ([#15](https://github.com/ar4mirez/spinel/issues/15)). Lookup takes a class; mapping a
  `Value` to one is that slice's job.
- **Per-class serials, and inline caches.** The global method cache and a serial that
  invalidates it are here, because a method table without invalidation is a bug waiting for
  its first caller. Sharpening the serial to one per class is
  [#9](https://github.com/ar4mirez/spinel/issues/9); the per-call-site cache that reads it
  is #10's, because there are no call sites yet.
- **Ractors.** Classes are per heap. engine.md makes them shared objects behind the main
  Ractor's class lock, and [#118](https://github.com/ar4mirez/spinel/issues/118) is the
  slice with a second Ractor to share them with.

## Users

| User | Needs from this slice |
|---|---|
| [#10](https://github.com/ar4mirez/spinel/issues/10) bytecode, interpreter | A method lookup that is one hash probe on the hot path, and a serial an inline cache can key on |
| [#11](https://github.com/ar4mirez/spinel/issues/11) calling convention, `super` | The `owner` of a found method, so `super` resumes at the right point in the chain |
| [#13](https://github.com/ar4mirez/spinel/issues/13) `class`/`module` bodies, singletons | `define_class`, `define_module`, and singleton classes that already exist and are already lazy |
| [#15](https://github.com/ar4mirez/spinel/issues/15) `core/*.rb` | Shells to reopen with the right ancestry, so `Integer.ancestors` is not wrong before a line of Ruby runs |
| [#9](https://github.com/ar4mirez/spinel/issues/9) per-class serials | A serial and a cache to sharpen, rather than a design to invent |
| [#151](https://github.com/ar4mirez/spinel/issues/151) shapes | A class object to hang a shape tree off, and the reserved header bytes to put a shape id in |

## Requirements

### R1 — A class owns a *run* of the chain, not a list of mixins

Every class and module owns the modules prepended to it, then itself, then the modules
included in it. A class's ancestry is its own run followed by its superclass's. Two things
fall out for free: `include` on a superclass is visible to every subclass with nothing
propagated, and `ancestors` is a walk rather than a linearisation.

The run is maintained by `include`/`prepend` rather than recomputed, because the order
depends on the state of the chain at each call and cannot be recovered afterwards:

```ruby
module M; end
module A; include M; end
module B; end
class C; include M; include B; include A; end   # [C, A, B, M]
```

`A` brings `M`, but `M` is already there, so `A` goes in front of `B` while `M` stays
behind it. Replaying `[M, B, A]` against `A`'s final contents gives `[C, A, M, B]`.
Ruby's answer is the first one, so a mixin log is not a representation of this.

### R2 — The four splice sites are the whole of `include_modules_at`

CRuby's insertion routine differs between its callers in four numbers, and this is them:

| caller | inserts at | scan window | searches the superclass |
|---|---|---|---|
| `include` | behind the class | the whole run | yes |
| `prepend` | in front of the run | in front of the class only | no |
| an `include` propagated to an includer | behind its copy of the module | after that copy | yes |
| a `prepend` propagated to an includer | in front of its copy | in front of that copy | no |

Two consequences that a whole-chain search would get wrong, and that the table pins:

- `include M; prepend M` produces **two** `M`s. `prepend`'s scan stops at the class, so it
  never sees what `include` put behind it.
- `include A` where the target already has one of `A`'s modules splices around it — the
  insertion point walks *past* a module it finds in place, which is what leaves
  `[C, A, B, M]` above rather than `[C, A, M, B]`.

### R3 — A module that gains a mixin later reaches back

Ruby 3.0's [Feature #9573](https://bugs.ruby-lang.org/issues/9573): `M2.include M1` after
`M3.include M2` still puts `M1` in `M3.ancestors`. ruby/spec pins the exact order in two
places — `Module#ancestors` "returns a module that is included later into a nested module
as well" and `Module#include` "preserves ancestor order".

Each module keeps a **flat** list of everyone whose run holds it, not just its direct
includers, and a later mixin patches each of them at *their* copy of the module. Flat is
what makes it one pass instead of a recursive walk, and patching at each includer's own
anchor is what puts the new module next to the one that brought it rather than next to
the class.

### R4 — Method tables are Rust, so the class table is a root source

A `Vec`- or `HashMap`-backed method table hanging off a class is memory the collector does
not trace. #8's options were to put method entries in object slots or to add the table to
`Heap::mark`; this takes the second, because a method table needs a hash map and a hash
map needs `Hash`, which is phase 2.

So `Heap::mark` gains a second root source: every class object, and every method body.
`shade` now takes the mark stack rather than `&mut Heap`, which makes that walk two
disjoint field borrows rather than a table moved out and put back.

### R5 — Singleton classes are allocated on the first ask

Never at class creation: a program that defines a thousand classes and takes the singleton
of none should allocate no metaclasses. A class's singleton inherits from its superclass's,
`BasicObject`'s from `Class`, and a module's from `Module` — the twist that puts `Class`,
`Module`, `Object`, `Kernel`, and `BasicObject` at the end of every metaclass's ancestors.

An ordinary object's singleton *becomes* its class, exactly as in Ruby, so the header write
is the whole mechanism and asking twice returns the same class because the second ask finds
a singleton already there.

### R6 — Bootstrap is a data table, in one order

engine.md's boot order step 1 lists the shells the VM creates. They are defined in
`Builtin`'s declaration order, which is what makes `Builtin::Object.id()` a cast rather
than a lookup, and `bootstrap` asserts the two agree rather than trusting them to.

`Comparable`, `Enumerable`, and `Numeric` join engine.md's list. The same sentence asks for
"the right ancestry", and without them `Integer.ancestors` and `Array.ancestors` are wrong
from the first commit — and `core/*.rb` cannot fix an ancestry it is loaded into.

### R7 — One serial, and a cache that is emptied rather than stamped

Every change that can move a method — a definition, a removal, an `include`, a `prepend` —
bumps a serial and empties the global method cache. Emptied rather than stamped-and-left,
because a stale entry can name a body that `remove_method` just dropped, and the cache
would be the only thing still keeping it from the collector.

The serial is one per class *table*, where engine.md describes one per class. Correct, and
coarser: a definition anywhere evicts every cached lookup. See the open decisions.

## Definition of done

| From the issue | Where |
|---|---|
| Rust unit tests verify `include`/`prepend` ordering against ruby/spec's documented cases | `tests/ancestors.txt`: 42 cases, 52 expectations, every one measured from Ruby 4.0 rather than written by hand. ruby/spec's own cases are in it by name — `ruby_spec_recursively_includes_new_mixins`, `ruby_spec_ignores_modules_already_included_by_mutual_inclusion`, `a_superclass_chain_follows_the_class` (the `Module#ancestors` opener), `a_re_include_after_the_module_grew_preserves_the_order` |
| Singleton classes allocate lazily | `singleton_classes_are_allocated_on_the_first_ask`: asking whether one exists allocates nothing, and asking for `B`'s allocates exactly the four its ancestors contain |
| Ancestor chain is correct for diamond and repeated-include cases | `a_diamond_keeps_one_copy_of_the_shared_module`, `a_repeated_include_changes_nothing`, `a_repeated_prepend_changes_nothing`, `ruby_spec_ignores_modules_already_included_by_mutual_inclusion` |

`cargo test -p spinel-vm`: 46 passing, 12 new — 11 unit tests and the table runner.
`cargo miri test -p spinel-vm` is green. The `ancestors oracle` CI job re-measures the
table against Ruby 4.0.

## Tasks

| | Task | Check |
|---|---|---|
| T1 | Bootstrap classes with Ruby's ancestry | `the_bootstrap_hierarchy_is_rubys` |
| T2 | Class objects, and a header class pointer that resolves back | `an_instance_resolves_back_to_its_class` |
| T3 | Lazy singleton classes for classes and modules | `singleton_classes_are_allocated_on_the_first_ask` |
| T4 | Singleton class of an ordinary object | `an_objects_singleton_replaces_the_class_in_its_header` |
| T5 | Method tables, lookup, removal, `prepend` precedence | `lookup_walks_the_chain_and_a_prepended_module_wins` |
| T6 | Global method cache and its invalidation | `the_method_cache_is_emptied_by_anything_that_can_move_a_method` |
| T7 | `include`/`prepend` refusals | `a_class_argument_and_a_cycle_are_both_refused` |
| T8 | The class table as a root source | `the_collector_traces_class_objects_and_method_bodies` |
| T9 | The serial | `the_serial_moves_for_every_change_that_can_move_a_method` |
| T10 | The CRuby oracle and its table | `scripts/ancestors-oracle.rb`, `tests/ancestors.txt` |
| T11 | Spinel held to the table | `spinel_agrees_with_the_ruby_ancestors_table` |
| T12 | The oracle in CI | `.github/workflows/ci.yml` |
| T13 | engine.md carries the model that exists | The "Classes and ancestor chains" section |
| T14 | The roadmap and the tracker agree | One phase-1 bullet became three, matching #8, #151, and #9 |

## What the audit caught

- **The collector could have lost every class.** Walking the class table needed `&self`
  while `shade` needed `&mut self`, and the first version moved the table out for the walk
  and put it back. A panic anywhere in that walk — a `Value` that failed a debug assertion,
  say — would have left the heap holding a `Default` table, which is to say no classes at
  all, and the panic would have been blamed for whatever happened next. `shade` now takes
  the mark stack, so the walk is two disjoint field borrows and there is nothing to put
  back.
- **Every class leaked a root.** `define_class` allocated in the caller's scope, so a
  program defining classes at load time grew the root stack by two entries per class, for
  ever, on top of the table that was already rooting them. Allocation now happens in a
  nested scope that drops before the call returns.
- **A wrong class pointer answered `None` instead of failing.** `class_of` used `?` on the
  slot read, so an object whose header pointed at something that was not a class read as
  "this object has no class" — the same answer an immediate gives. It is an `expect` now:
  the two cases are not the same and only one of them is a bug.
- **A cycle three modules apart.** `A.include B; B.include C; C.include A` is a cycle only
  because `B`'s include was propagated back into `A`. Comparing the two arguments would
  miss it; one `contains` on the materialised chain catches it, which is a property of R1
  worth having a test rather than a comment.
- **Two prepends onto a module came back reversed from anything that included it.**
  Propagating a prepend anchored it at the module's own position, so the second prepend
  landed *behind* the first in the includer while sitting in front of it in the module:
  `M` read `[P2, P1, M]` and `C` read `[C, P1, P2, M]`. The site is the start of the
  module's run — its index walked back over the prepends already propagated there, which
  is the flat form of the iclass head CRuby inserts behind. Found by adding three cases to
  the table after the first 32 passed, which is the argument for the table being cheap to
  extend: the bug was in code that already had a green test for the case one step simpler.
- **`prepend` raised the wrong message.** One `Cyclic` variant meant a cyclic
  `prepend` reported "cyclic include detected". ruby/spec has both cases by name, and
  engine.md's rule is that Ruby's message text is part of the behaviour, so the error
  carries which method was called. The same change removed a `prepending: bool` from three
  signatures, which is the kind of argument that reads wrong at every call site.
- **Reading `class.c` would have produced the wrong engine.** The first model of `prepend`
  had it searching the whole chain, which is what the C appears to say. Ruby says
  `include M; prepend M` yields two `M`s. Four of the table's cases exist because a
  measurement disagreed with a reading, and they are the reason the oracle is a CI job
  rather than a one-off script.

## What the triage found

The roadmap folded this slice, the global method cache, and class serials into one bullet
while the tracker splits them across #8 and #9; the bullet is three now, and #9's scope is
narrowed on the issue to the descendant walk it still owes. Two of this PRD's own
cross-references were wrong — #9 is the method cache, not shapes, and #14 is the regex
engine, not constants. Chasing the first of those turned up work nothing tracked at all:
engine.md commits to hidden-class shapes and no issue or roadmap bullet existed for them,
which is [#151](https://github.com/ar4mirez/spinel/issues/151) and a new bullet.

## Numbers

None worth recording. Every operation here is a load-time operation: bootstrap is 15
classes, and the deepest case in the table has six. The number that will matter is method
lookup under a real workload, and the thing to measure it with is #10's interpreter.

## Open decisions for the owner

1. **One serial for the whole table, not one per class.** engine.md describes a serial
   that bumps on a definition in the class or its ancestors. A shared one is correct and
   coarser: defining a method anywhere evicts every cached lookup, which is free at load
   time and wrong in a program that defines methods while running. Per-class serials need a
   subclass list and a descendant walk per definition, and that is
   [#9](https://github.com/ar4mirez/spinel/issues/9) — which this slice leaves narrower
   than it found it rather than closing.
2. **The method cache is unbounded.** CRuby's global cache was a fixed-size direct-mapped
   table that evicted rather than grew. This one is emptied by every definition, so it
   cannot outgrow the `(class, name)` pairs a program actually calls between two of them —
   but "between two definitions" is a program-shaped bound, not a number.
3. **A `ClassId` is an index, not branded to its heap.** Same shape as PRD 0007's third
   open decision about `Handle`, and the same trade: two heaps in one Rust function could
   pass an id between them, the index is bounds-checked, and the failure is a panic or the
   wrong class rather than memory unsafety.
4. **A class is named by the string it was defined with**, where Ruby derives
   `Module#name` from the constant it is first assigned to and leaves it `nil` until then.
   Nothing can assign a constant yet, so the two agree on every case in the table; they
   stop agreeing the moment #13 lands, and #13 is the slice that should take the name away
   from `define_class`.

## Follow-ups

- Per-class serials, and the subclass list they need — [#9](https://github.com/ar4mirez/spinel/issues/9),
  whose scope this slice narrows to exactly that.
- `Module#name` as Ruby computes it, and the anonymous-to-named transition on first
  assignment to a constant — [#13](https://github.com/ar4mirez/spinel/issues/13), which is
  the slice that has constants to assign to.
- Reclaiming an unreachable class. Every class object is a permanent root today, because
  deciding that one is unreachable needs the constant table to say what still names it.
  A program that generates classes at runtime — `Struct.new` in a loop — grows the heap by
  one class object each time until then.
- `alias_method`, `Module#instance_methods`, `method_defined?`, `undef_method`. Each is a
  few lines on the tables that exist; none has a caller until #10 can express one.

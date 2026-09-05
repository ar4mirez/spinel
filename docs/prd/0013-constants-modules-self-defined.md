# PRD 0013 — Constants, modules, `class`/`module` bodies, `self`, singleton classes, `defined?`

Tracks [#13](https://github.com/ar4mirez/spinel/issues/13). Milestone: Phase 1: a VM that
runs `language/`. `P0`, `size:M`, `area:engine`.

## Objective

[#11](https://github.com/ar4mirez/spinel/issues/11) gave the VM a calling convention, and
with it the ability to run a method. What it cannot do is *name* anything. There is no
constant, so no fixture class in the corpus can be reached; no `class` body, so no fixture
class can be written; `def` defines on `class_of(self)`, which is right at the top level by
accident and wrong everywhere else; and `defined?` is `Unsupported`.

This slice is the naming half of the language. After it, a Ruby file can declare a class,
put methods in it, reach it by name from another file's lexical scope, and ask whether a
name means anything.

At the baseline below, four blocked reasons cover it — a constant (760), a constant path
(215), `defined?` (78), a class or module body (63) — **1,116 of `language/`'s 2,475
blocked examples**, the largest remaining group by a factor of two.

### The honest shape of the target

The issue's definition of done reads "`language/` minus `regexp/` newly passes". Taken
literally that is not reachable from this slice, and the reason is not naming. The corpus
also blocks on:

```ruby
# variables_spec.rb, yield_spec.rb, and most fixture-based files
@a = VariablesSpecs::ClassA.new    # instance variables are #151's shapes
-> { @a.z }.should raise_error(...)  # exception objects are #12's
```

419 examples block on assigning an instance variable and 62 on `begin`/`rescue`. Neither
is in this slice's title, and building either here would build half of another slice badly.

So the measurable claim is narrower and checkable: **naming stops being the blocker.**
After this lands, no example in the corpus is blocked on "a constant", "a constant path",
"a class or module body", "`defined?`", or "a singleton method definition", and the
`blocked → passed` delta is stated in the PR with every remaining reason named and
attributed to a later slice.

## Non-goals

- **Instance variables.** `@a` and `@a = 1` stay `Unsupported`. They are
  [#151](https://github.com/ar4mirez/spinel/issues/151)'s shape tree; a `HashMap` per
  object now is a second storage scheme that #151 would have to delete. This is the single
  biggest thing standing between this slice and "`language/` passes", and it is named in
  the numbers rather than smuggled in here.
- **Class variables and global variables.** `@@a` and `$a` stay `Unsupported`, including
  under `defined?`. Answering `defined?(@@a)` with `nil` because the VM has no class
  variables would be a *wrong answer that passes a spec*, which is exactly the failure
  mode `Unsupported` exists to prevent.
- **`defined?` swallowing an exception.** Ruby's `defined?` rescues anything raised while
  evaluating a receiver and answers `nil`. Catching needs #12's unwinder; until then such
  an example is reported blocked, never `nil`. See R9.
- **Reflection.** `Module.nesting`, `const_get`, `const_set`, `constants`,
  `const_defined?`, `Module#name`, `singleton_class` as a *method* — all
  [#28](https://github.com/ar4mirez/spinel/issues/28)'s. This slice builds the machinery
  they will read, and exposes none of it as Ruby methods.
- **Autoload.** [#39](https://github.com/ar4mirez/spinel/issues/39).
- **Constant reassignment warnings**, `private_constant`, `deprecate_constant`.
- **Per-class constant caches.** Constant lookup walks the chain every time. The
  invalidation serial #8 already bumps is where a cache goes; the benchmark that justifies
  one arrives with the JIT.

## Users

The compiler, the interpreter, and `spec/harness`. Also every later slice: `core/*.rb`
cannot define a single class until this lands, so #15 and the whole of phase 2 sit behind
it.

## Requirements

### R1 — Constants live on the class table entry, next to the methods

`Entry` grows `constants: HashMap<SymbolId, Value>`, and `Classes::each_root` traces it.
Not a separate registry: a constant is owned by exactly one module, it dies with that
module, and the table that already roots the module is the table that should root what it
holds.

### R2 — Lexical scope is a runtime chain, because only the runtime knows the classes

Constant lookup needs the *lexical* chain of modules, and the compiler cannot supply it:
it knows `class C` is nested one level deeper, but `C` is a `ClassId` that does not exist
until the body runs. So the chain is built at runtime, exactly as CRuby's cref is.

`Crefs` is a per-heap arena of `(ClassId, Option<CrefId>)` nodes — a linked list, shared
by every frame in the same lexical scope, `Copy` at every use site. `CrefId::ROOT` is
`Object` and is node 0, seeded by `bootstrap`.

An arena rather than a heap object because a cref is not reachable from Ruby, is never
collected independently of the class it names, and is read on the hot path of every
constant reference. Its `ClassId`s are already rooted by the class table.

### R3 — A cref is carried by a frame, captured by a method, and captured by a proc

Three carriers, because a constant reference resolves in the scope it was *written* in,
not the one it runs in:

| carrier | where the cref comes from |
|---|---|
| frame | the frame that pushed it |
| method | the cref at `def`, stored on `Method` |
| proc | the cref at `MakeProc`, a sixth slot |

```ruby
module A
  X = 1
  class B
    def m = X          # A::X, though B.ancestors never reaches A
  end
end
```

`Method` gaining a `cref` field is what makes `m` above answer `1`. `Proc` gaining a slot
is what makes `class C; X = 1; [1].each { X }; end` answer the same.

### R4 — Lookup is lexical, then ancestors, then `Object`, and each step is measured

Bare `X`, in order:

1. Each module in the cref chain, innermost first, **own table only**.
2. `ancestors(cref.innermost)`, in order.
3. `Object`, if step 2 did not already reach it — which it does for a class and does not
   for a module.

`A::X` is step 2 alone, rooted at `A`, with no lexical scope — and with `Object` skipped
even though it sits in the chain, which is Ruby 2.5's change. The skip is narrower than
"no fallback": `Object` alone is passed over, and only when it is not itself the receiver,
while `Kernel` and `BasicObject` are searched like any other ancestor. `::X` is step 2
rooted at `Object`, where the skip therefore does not apply. `::X` is step 2 rooted at `Object`.

Each of these is a line in `crates/spinel-vm/tests/eval.txt`, generated from
`ruby 4.0.6` by `scripts/eval-oracle.rb`, in the shape #8 and #10 already use for
`ancestors.txt`. The ordering rules here are documented nowhere and read wrong from
`variable.c`; measuring them is the only way to be sure.

### R5 — `NameError` and `TypeError` carry Ruby's message text

`uninitialized constant Foo`, `uninitialized constant Bar::Foo`, `1 is not a class/module`,
`superclass mismatch for class C`, `superclass must be an instance of Class (given an
instance of Integer)`, `C is not a class`. Dead text until #12 raises it, free to write
now, and #12 should not have to rediscover the format — the same argument R9 of PRD 0011
made for arity messages.

### R6 — `class`/`module`/`class <<` are one instruction over one table entry

`Insn::OpenClass(u32)` indexes `Iseq::class_defs`. The entry carries the name symbol, the
body child index, the kind (`Class`, `Module`, `Singleton`), and two flags saying what is
on the stack: a `cbase` for a scoped path (`class A::B`), a superclass for `class C < D`.

Flags in the table entry rather than in the instruction, for the reason R1 of PRD 0011
gave: `Insn` stays `Copy` and 16 bytes, and the decode is a field read either way.

Defining is *find-or-create against the cbase's own table*, which is what CRuby's
`rb_const_defined_at` does and what makes this true:

```ruby
class P; class Inner; end; end
class Q < P
  class Inner; end     # Q::Inner — a new class, not a reopening of P::Inner
end
```

### R7 — `def` defines on the cref, not on `class_of(self)`

Today `DefineMethod` pops `self` and defines on `class_of(self)`. At the top level `self`
is `main`, `class_of(main)` is `Object`, and the answer is right by coincidence. In a class
body `self` is `C` and `class_of(C)` is `Class`, so every method would land on `Class`.

`Insn::DefineMethod` takes no receiver and defines on `cref.innermost`.
`Insn::DefineSingleton` pops one and defines on `singleton_class_of(receiver)`, which is
`def self.foo` and `def obj.foo`. Two instructions rather than a flag, because the operand
stacks differ.

### R8 — `self` is the class in a class body and the singleton in a `class <<` body

`OpenClass` pushes a frame whose receiver is the module it just opened. That is the whole
of `self` semantics for this slice, and it is what makes `def self.foo` inside `class C`
reach `C`'s singleton rather than `main`'s.

### R9 — `defined?` answers from the node kind, and refuses what it cannot know

Ruby's answers are a table, not a rule, and several entries are surprising enough to be
worth measuring rather than reasoning about:

| expression | answer |
|---|---|
| `nil` / `true` / `false` | `"nil"` / `"true"` / `"false"` — *not* `"expression"` |
| `1`, `"s"`, `:s`, `[1]`, `->{}`, `if`, `while`, `1 && 2` | `"expression"` |
| `!true`, `1 + 1`, `1.to_s` | `"method"` — `!` and `+` are methods |
| `a = 1`, `@a = 1`, `A = 1` | `"assignment"`, and the assignment does not happen |
| `self` | `"self"` |
| `yield` | `"yield"` with a block, `nil` without |
| `A`, `A::B`, `::A` | `"constant"` or `nil` |
| `foo`, `x.foo` | `"method"` or `nil` |

Most are compile-time and become a literal push. The four runtime ones are
`Insn::DefinedConst`, `Insn::DefinedMethod`, `Insn::DefinedSelfMethod`, and
`Insn::DefinedYield`.

Two rules hold the safety property:

- **No side effects the answer does not need.** `defined?(R.ok)` must not call `ok`;
  `defined?(R.ok.to_s)` must call `ok`, because Ruby evaluates the receiver chain and
  checks only the last name. Measured, not assumed.
- **A kind the VM cannot answer is `Unsupported`, never `nil`.** `defined?(@a)`,
  `defined?(@@a)`, `defined?($a)`, `defined?(super)`. Ruby answers `nil` for an undefined
  one, so a VM without instance variables would "pass" `defined?(@nope).should be_nil`
  while failing the spec next to it. Blocked is the honest column.

### R10 — The table is generated from CRuby, and CI re-runs every pass

The cases go in `crates/spinel-vm/tests/eval.txt`, which `scripts/eval-oracle.rb`
already generates and re-checks against CRuby. A separate `constants.txt` and a
second oracle script were the plan and would have been a second copy of one that
works: Ruby's `;` makes a whole class hierarchy fit on one line, so the existing
one-line-per-case format holds these without a change. The cases the table
cannot hold — the ones that raise — are assertions in `tests/eval.rs`.
`scripts/verify-passes.rb` re-runs every example this slice moves into the passing column
against `ruby 4.0.6`; it is the check that a new `passed` count is real. Any harness change
this slice makes lands in `verify-passes.rb` in the same commit, or CI reports false passes.

## Definition of done

- [x] No example in the corpus is blocked on "a constant", "a constant path", "a class or
      module body", "`defined?`", or "a singleton method definition". Zero matches across
      all 3,835 files.
- [x] Constant lookup follows lexical scope, then ancestors, with `Object` last, measured
      line by line against CRuby in `tests/eval.txt`.
- [x] `defined?` returns the right string for every node kind it accepts, and refuses
      every kind it cannot answer — never a wrong `nil`.
- [x] `language/{constants,class,module,metaclass,defined}_spec.rb`: delta stated below,
      remaining reasons named and attributed.
- [x] `cargo test` green; `cargo clippy --all-targets` clean; miri green on `spinel-vm`.
- [x] `ruby scripts/verify-passes.rb spec/ruby` agrees on every passing example (264).
- [x] engine.md gains the "Constants and lexical scope" section it was missing, and
      roadmap.md's check line for this slice matches what landed.

## Tasks

1. `Entry.constants`, `Classes` const get/set/lookup, GC tracing.
2. `Crefs` arena, `CrefId`, `ROOT` seeded at bootstrap.
3. `Method.cref`; `Frame.cref`; `Proc`'s sixth slot.
4. `Insn::GetConst`/`SetConst`/`DefinedConst` with a `ConstScope`, and `Iseq::class_defs`.
5. `Insn::OpenClass`; interpreter find-or-create, superclass check, frame push.
6. `Insn::DefineMethod` on the cref; `Insn::DefineSingleton`.
7. Compiler: `Class`, `Module`, `SingletonClass`, `ConstPath`, `VarRef::Const`, const
   targets, `def self.x`, `defined?`.
8. `scripts/constants-oracle.rb` + `tests/constants.txt` + `tests/constants.rs`.
9. Docs: engine.md's constants section, roadmap check line, this PRD's numbers.

## Numbers

### The five files this slice names

| file | before | after |
|---|---|---|
| `constants_spec.rb` | 0 passed · 100 blocked | 0 passed · 100 blocked |
| `class_spec.rb` | 0 passed · 45 blocked | 0 passed · 45 blocked |
| `module_spec.rb` | 0 passed · 16 blocked | 0 passed · 16 blocked |
| `metaclass_spec.rb` | 0 passed · 21 blocked | 0 passed · 21 blocked |
| `defined_spec.rb` | 0 passed · 257 blocked | **17 passed** · 240 blocked |
| `def_spec.rb` | 12 passed · 60 blocked | **16 passed** · 56 blocked |

`language/`: `216 → 240` passing, `2475 → 2451` blocked, 0 failed.
Whole corpus: `238 → 264` passing, `23532 → 23506` blocked, 0 failed.

**Four of those files stayed at zero, and that is the honest headline.** They
are not blocked on naming any more — the DoD's five phrases return zero matches
across all 3,835 files — they are blocked on the two things every one of them
reaches through:

```
    427  uninitialized constant ScratchPad     mspec's own helper; #145
    100  `should` on a Proc                    `-> { }.should raise_error`; #12
     92  `mock` on an Object                   mspec's mocking; #145
     45  uninitialized constant ClassSpecs     fixtures need `require`; #39
     41  uninitialized constant DefinedSpecs   ditto
```

`constants_spec.rb` is 100 examples and 35 of them are `ConstantSpecs::*`
fixtures; the rest are `-> { }.should raise_error(NameError)`. Neither is a
constant lookup this slice got wrong, and both unblock without touching the
code this slice wrote.

### What blocks `language/` now, and whose slice it is

```
    438  assigning an instance variable        #151 shapes
    427  uninitialized constant ScratchPad     #145 mspec
    344  a regexp                              #14
    100  `should` on a Proc                    #12
     92  `mock` on an Object                   #145
     89  a multiple assignment                 #154
     74  `new` on a built-in class             #15 core/*.rb
     66  `begin`/`rescue`                      #12
     51  `defined?` of a name never loaded     #39 require
     48  a local from an enclosing scope       harness boundary, see PRD 0011
```

Not one of them is naming. The five DoD phrases — `a constant`, `a constant
path`, `a class or module body`, `` `defined?` ``, `a singleton method
definition` — return **zero** matches corpus-wide.

### Checks

- `cargo test`: green, including **45 new measured cases** in `tests/eval.txt`
  covering lookup order, class and module bodies, `self`, singleton classes, and
  `defined?`, all generated from `ruby 4.0.6` by `scripts/eval-oracle.rb`.
- `cargo +nightly miri test -p spinel-vm --lib`: 58 passed, including the two new
  GC and scope-chain tests.
- `cargo clippy --all-targets`: no warnings.
- `ruby scripts/eval-oracle.rb --check` and `ancestors-oracle.rb --check`: agree.
- `ruby scripts/verify-passes.rb spec/ruby`: **all 264 passing examples re-run on
  ruby 4.0.6 and agree.** No false passes anywhere in the corpus.

### A scope addition, named rather than smuggled

`Class#new` is not in the issue's title, and it is here. Three reasons, in order
of weight: without it a class can be defined and never instantiated, so this
slice's own subject is untestable and its `eval.txt` table could not be written;
it is allocation plus dispatch, which engine.md reserves for a Rust primitive;
and it was blocking 132 examples on its own. It allocates a zero-slot object and
runs `initialize` if there is one, as a frame push rather than a re-entrant
`eval` — PRD 0011's R7 — with a `keeps_receiver` flag so `new` answers the object
whatever `initialize` returned. It refuses every built-in class but `Object` and
`BasicObject`; see the audit below for why that is not caution.

## What the audit caught

Eight. The first four the checks found, the last four came from reading the code
against Ruby after CI was already green:

1. **A metaclass the table knew about and the header did not.**
   `HandleScope::singleton_class` linked `entry.singleton` but never rewrote the
   class object's header, so `class C; def self.m; end; end; C.m` raised
   `NoMethodError` — dispatch reads the header, found `Class`, and looked there.
   `singleton_class_of` had always done the write for ordinary objects; the class
   path had never needed it because nothing could call a singleton method yet.

2. **Lazy metaclasses break inheritance.** With the header fixed, `class B < A`
   still could not reach `A`'s `def self.m`: `B` had never been asked for a
   singleton, so it still pointed at `Class`. CRuby builds a class's metaclass in
   `rb_define_class` for exactly this reason. The `class` keyword now does too;
   `define_class` stays lazy, so the test that asserts laziness still holds.

3. **`Proc.new` built something that was not a `Proc`.** `Class#new` allocated a
   zero-slot object wearing `Proc`'s class, and `Proc#lambda?` read slot 3 of it
   and panicked — on the *whole corpus*, in the release build. Two bugs: an
   unguarded slot read that had been unreachable until something could forge a
   receiver, and `Class#new` having no opinion about built-ins. Both fixed;
   `lambda?` is now guarded like `arity` already was.

4. **`def m(_, _)` bound two arguments to one slot.** Ruby allows a repeated
   parameter name when it starts with an underscore, and Prism's scope list holds
   one entry for the pair, so `ParamSpec` claimed two slots the frame did not
   have. A `debug_assert` from #11 caught it the moment class bodies let the
   compiler reach that `def` — the assert working exactly as its comment said it
   would. Pre-existing, and #13's to fix because #13 is what made it reachable.

5. **`defined?` was answering `nil` for names it had merely never loaded**, which
   produced 16 spec *failures* and, worse, would have produced silent passes on
   `defined?(SomeFixture).should be_nil`. R8 of PRD 0011 had already settled the
   principle one layer up — an unknown method raises rather than answering `nil`
   — and the same answer applies here. Making a miss `Error::Unknowable` cost 23
   examples that had been "passing" and took failures to zero.

6. **`defined?` recurses, and not where the syntax suggests.**
   `defined?([1, NoSuch])` is `nil` but `defined?(if NoSuch then 1 end)` is
   `"expression"`; a call's receiver *is* evaluated and its arguments are *not*.
   None of that is guessable, and the first draft had it wrong in three places.
   Measured against `ruby 4.0.6` and written down in `Compiler::defined`.

7. **`can't define singleton` has no receiver in it.** The first draft wrote
   `can't define singleton for 1`. Ruby's text is four words. ruby/spec asserts
   on it, so a plausible-looking message is a failure waiting for #12.

8. **A qualified lookup skips `Object`, and only `Object`.** `S::TOP` for a
   top-level `TOP` answered `1` where Ruby raises `NameError`: the first draft
   read "no `Object` fallback" as "the ancestor walk already handles it", and the
   walk reaches `Object` like any other ancestor. Measuring it showed the rule is
   narrower still — `Kernel` and `BasicObject` are *not* skipped, and `Object` is
   not skipped when it is the receiver. Caught by an assertion written for the
   error-message table, which is the argument for writing the raising cases down
   even though `eval.txt` cannot hold them.

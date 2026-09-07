# PRD 0027 — `super`, global variables, source-position keywords, and `case`/`in`

Issues: [#187](https://github.com/ar4mirez/spinel/issues/187),
[#166](https://github.com/ar4mirez/spinel/issues/166),
[#174](https://github.com/ar4mirez/spinel/issues/174),
[#165](https://github.com/ar4mirez/spinel/issues/165) · Phase 1 · `area:engine`

## Objective

Four compiler refusals, taken in one pass because they are the four largest
`language/` gaps left that are not a missing core class. Each is independently
verifiable and each is measured the same way: the blocked-reason it names must
reach zero, and the ruby/spec delta is stated per directory.

| issue | refusal | examples blocked (baseline) |
|---|---|---|
| #174 | `a source-position keyword is not compiled yet` | 478 |
| #166 | `a global variable` / `assigning a global variable` / `` `defined?` on a global variable `` | 288 + 85 + 9 = **382** |
| #187 | `` `super` `` — plus the two fixtures it stops compiling | 49 + `SuperSpecs` 52 + `EnumerableSpecs` 394 = **~495** |
| #165 | `` `case`/`in` pattern matching `` / `pattern matching` | 87 + 14 = **101** |

## Baseline

Measured on `main` at 34d93dc.

| | |
|---|---|
| Rust tests | 263 passing · 0 failing |
| ruby/spec corpus | 3835 files · 25,624 examples · **1,608 passed** · 0 failed · 22,145 blocked · 1,871 skipped |

## Decisions

### #174 — `__LINE__` is a *parse*-time constant, not a compile-time one

The issue calls both `__FILE__` and `__LINE__` compile-time constants. Only
half of that is reachable: a [`Span`] is a byte offset, and turning one into a
line number needs the source bytes, which the compiler does not have and eight
call sites would have to start passing it.

The parser does have them. `ExprKind::SourceFile` already carries its payload
(`Bytes`), so `ExprKind::SourceLine` gains one too — `SourceLine(u32)`, filled
by `lower.rs` from the offset table it can build once per parse. The compiler
then emits a `PushInt` and a `PushLit`, and no compiler signature changes.

`__FILE__` still needs a path Prism is never told: `ruby-prism` 1.9 exposes no
`ParseOptions`, so `filepath()` is always empty. `spinel_parse::parse_file`
takes one and fills the node; `parse` keeps answering `"(eval)"`, which is what
CRuby answers for source with no file.

### #174 — `__ENCODING__` is deferred, explicitly

It needs an `Encoding` object, and `uninitialized constant Encoding` is
separately 630 examples. It stays refused, under its own reason
(`` `__ENCODING__` needs the Encoding class ``) so the ranking counts it apart
from the two that landed.

### #166 — the table is keyed by name, and the specials never enter it

`Heap` gains `globals: HashMap<SymbolId, Value>` and a fourth root source in
`Heap::mark`, beside `classes`, `regexps` and `last_match`. Three instructions
read and write it.

The regexp specials are *not* routed through it. `match_ref` already claims
`$~`, `$&`, `` $` ``, `$'` and `$1`..`$n` for `Insn::LastMatch`, and it is
checked first, so an assignment table never sees a name no assignment writes.
Measured, because the two disagree in a way reading will not tell you:

```
defined?($~)  #=> "global-variable"    always
defined?($&)  #=> nil                  until a match sets one
defined?($1)  #=> nil                  same
```

An unset ordinary global reads `nil` and is `defined?`-nil; assigning `nil` to
one makes it defined. That is the whole reason the table stores presence rather
than truthiness.

### #187 — `super` is a *lookup*, so the frame has to carry its owner

`Method` already records `owner: ClassId`. What is missing is the frame: a
`Call` knows its receiver and its `Iseq` but not which class the method it is
running was found on, and `super` starts one past exactly that.

`Call` gains `owner: Option<ClassId>` and `method: Option<SymbolId>`, and
`dispatch` fills both from the resolved `Method`. `Target::Super { owner }`
then walks `classes.ancestors(class_of(receiver))`, finds `owner`, and searches
on. Starting from the *superclass* instead is the case the issue warns about,
and it is wrong for a method a module defines:

```ruby
module M; def m = "M" + super; end
class A; def m = "A"; end
class B < A; include M; def m = "B" + super; end
B.new.m  #=> "BMA"      B.ancestors == [B, M, A, ...]
```

### #187 — zsuper reads the parameter locals, it does not snapshot them

Bare `super` forwards the arguments *as they stand now*, reassignments
included. Measured:

```ruby
class B < A; def m(a, b=2, *r, k: 9); a = a*10; super; end; end
B.new.m(1)  #=> [10, 2, [], 9]
```

So the compiler emits an ordinary `GetLocal` per parameter, in the shape the
`ParamSpec` declares — which gets the reassignment right for free, and needs no
new runtime concept. A block body inherits the enclosing method's parameter
list along with a scope depth, because `super` inside a block forwards the
method's arguments, not the block's.

### #165 — patterns lower to tests and jumps, not to a matcher object

No new opcode. Every pattern form is a comparison the VM already has — `===`
for a constant, `==` for a literal or a pin, a `deconstruct` send for an array
pattern, `deconstruct_keys` for a hash pattern — so the compiler emits the same
`Send`/`JumpUnless` shape a hand-written `if` chain would, and the interpreter
learns nothing new.

`NoMatchingPatternError` and `NoMatchingPatternKeyError` are `core/*.rb`, which
is where `CLAUDE.md` puts anything Ruby can express.

## Plan

1. #174 — `SourceLine(u32)` in the AST, filled by the parser; `parse_file`; compile both; defer `__ENCODING__` under its own reason.
2. #166 — `Heap::globals`, three instructions, `match_ref` checked first.
3. #187 — `Call::owner`/`Call::method`, `Target::Super`, zsuper forwarding from the `ParamSpec`.
4. #165 — pattern lowering, the two error classes in `core/`.
5. Oracle rows in `tests/eval.txt` for every case CRuby can be asked about stably; Rust tests for the rest.
6. ruby/spec delta per directory, stated below.

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| corpus passed | 1,608 | **1,798** |
| corpus failed | 0 | **0** |
| corpus skipped | 1,871 | 1,911 |
| `language/` passed | 922 | **1,083** of 2,735 |
| `verify-passes.rb` on `language/` | 922 agree | **1,083 agree** |
| Rust tests | 263 | **267** |

`+190` examples. Where each issue's share went:

| issue | reason | before | after |
|---|---|---|---|
| #174 | `a source-position keyword` | 478 | **0** — 3 to `` `__ENCODING__` ``, the rest to their next real blocker |
| #166 | the three global reasons | 382 | **0** |
| #187 | `` `super` `` | 49 | **0** |
| #187 | `uninitialized constant EnumerableSpecs` | 394 | **0** |
| #187 | `uninitialized constant SuperSpecs` | 52 | 52 — see below |
| #165 | `` `case`/`in` `` and `pattern matching` | 101 | **0** — 6 are `MatchWrite`, now named separately |

`language/pattern_matching_spec.rb` goes 0 → **85 of 114** passing, and
`language/super_spec.rb` 0 → 5.

### Definition of done

- [x] #174 — `__FILE__` is the path the `Iseq` was parsed with, `__LINE__` the source line; `__ENCODING__` deferred under its own reason
- [x] #166 — read, assign and `defined?`; the specials stay on `Insn::LastMatch`
- [x] #187 — `super`, `super(...)`, zsuper, the module lookup order, the block forwarding, and `super` inside a block
- [x] #165 — every pattern form the spec file measures, both one-line spellings, and the two error classes
- [x] `tests/eval.txt` gained 75 rows, every one measured by `scripts/eval-oracle.rb` against ruby 4.0.6
- [x] Rust tests for what the table cannot hold: paths, lines, and every raise

### Not delivered, and why

- **`language/fixtures/super.rb` still does not compile.** #187 names it in its
  definition of done, and it is not reachable from #187: the fixture uses
  `**kwrest` in 14 places, which is a different refusal — 101 examples corpus
  wide — that this slice does not own. `SuperSpecs` stays blocked. The other
  half of that requirement, `core/enumerable/fixtures/classes.rb`, does
  compile, and it was the larger one at 394 examples.
- **`NoMatchingPatternKeyError`.** A hash pattern that fails on a missing key
  raises it only when it is the sole clause — measured, two clauses give the
  general error — so reporting it needs the failure *reason* to travel out of
  the pattern and into the `case`. One example; tagged.
- **`__ENCODING__`.** 3 examples, waiting on the `Encoding` class, which is
  separately 632.

### Two things the unblocking revealed

Both are the shape `docs/roadmap.md` warns about: a slice that unblocks a lot
makes old failures visible, and they arrive in subsystems it never touched.

1. **38 predefined-global examples** were failing all along behind
   "assigning a global variable". All three families #166 explicitly defers —
   read-only and type-checked predefined globals, global aliases, and `$!` —
   so they are tagged with reasons naming #39. Four more were genuine
   core-library gaps and are fixed here: `Comparable#clamp`'s range check and
   `Hash#delete`/`#[]=` on a frozen receiver. The fourth needs a `String`
   subclass to hold bytes and is tagged.
2. **`verify-passes.rb` could be poisoned by its own corpus.** It gave each
   example a fresh *binding*, which isolates locals and nothing else.
   `case_spec.rb` contains `case (def foo; 'foo'; end; 'f')`, whose top-level
   `def` lands on `Object` and stays there; `super_spec.rb` then has a
   `Class.new(sup) { def foo; super; end }` find it, and the example asserting
   that `super` raises was reported as Spinel passing something Ruby does not.
   Nothing was wrong with Spinel — it gives every example its own `Heap` — and
   the script now forks per example, which is the same isolation. It could not
   surface before because the two files had no passing examples in common.

### Left for later

- **A `Hash`-valued row cannot live in `eval.txt`.** `Hash#inspect` still
  writes `{:a => 1}` where ruby 4.0 writes `{a: 1}`, so such a row would
  measure that rather than the pattern. `**rest` and `deconstruct_keys` are
  checked in `tests/eval.rs` instead.
- **`NoMatchingPatternError`'s message** is the subject's `inspect`; CRuby
  appends `": String === 1 does not return true"`. Every assertion in the spec
  file is a regex over the `inspect` half, so nothing turns on it yet.
- **A `Proc` that outlives its defining frame** loses its `super` target and
  raises "outside a method". Marked `ponytail:` in `push_proc_frame`, with the
  upgrade — two slots on the `Proc`, beside `PROC_HOME` — named.

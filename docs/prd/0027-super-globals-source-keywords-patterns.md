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

_Filled in as each step lands._

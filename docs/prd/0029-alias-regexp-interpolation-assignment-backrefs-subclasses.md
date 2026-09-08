# PRD 0029 — `alias`/`undef`, regexp interpolation, index and attribute assignment, back-references, and built-in subclasses

Tracks [#188](https://github.com/ar4mirez/spinel/issues/188),
[#189](https://github.com/ar4mirez/spinel/issues/189),
[#191](https://github.com/ar4mirez/spinel/issues/191),
[#192](https://github.com/ar4mirez/spinel/issues/192),
[#197](https://github.com/ar4mirez/spinel/issues/197) and
[#199](https://github.com/ar4mirez/spinel/issues/199).
Milestone: Phase 1: a VM that runs `language/`.

## Objective

Five compiler and runtime refusals and one harness decision. They are grouped
because four of the five are the *same shape of bug*: a construct the parser
lowers to its own node, which `compile.rs` then refuses in one arm — and
because three of them are each holding a **fixture file** hostage, so the direct
example count badly understates what they are worth.

| slice | direct | fixture leverage | total |
|---|---|---|---|
| #188 `alias`/`undef` | 45 | `ModuleSpecs` 219 + `KernelSpecs` 185 | **449** |
| #189 regexp interpolation | 27 | `ClassSpecs` 46 | **73** |
| #191 index/attribute assignment | 118 | `LangSendSpecs` 71 | **189** |
| #197 back-references | 52 | — | **52** |
| #199 built-in subclasses | 2 tagged | grows per unblocked fixture | **2** |
| #192 harness loop parameters | 310 | — | *decision, see below* |

## Baseline

Measured on `5877633`, the branch point:

```
3835 files · 25624 examples · 1823 passed · 0 failed · 21890 blocked · 1911 skipped
```

The refusals this PRD is measured against:

```
  310  a local variable from an enclosing scope is not compiled yet   (#192)
   83  an index assignment is not compiled yet                        (#191)
   45  `alias` or `undef` is not compiled yet                         (#188)
   35  an attribute assignment is not compiled yet                    (#191)
   27  regexp interpolation is not compiled yet                       (#189)
   22  `defined?` on a back-reference is not compiled yet             (#197)
   15  a regexp back-reference is not compiled yet                    (#197)
   15  assigning a regexp special variable is not compiled yet        (#197)

  219  NameError: uninitialized constant ModuleSpecs                  (#188)
  185  NameError: uninitialized constant KernelSpecs                  (#188)
   71  NameError: uninitialized constant LangSendSpecs                (#191)
   46  NameError: uninitialized constant ClassSpecs                   (#189)
```

## What was measured, and what it corrected

Every rule below came from running ruby 4.0.6 rather than from reading the
issue. Two measurements changed the design, and one caught a mistake in the
measurement itself.

### `$~` is frame-local, and a contaminated oracle says the opposite

The first pass measured `defined?($&)` in a single Ruby process where earlier
snippets had already matched, and reported `"global-variable"` *before any
match* — which would have made #197's `defined?` arm a constant. Re-measured
one fresh process per question:

| expression | before any match | after `"a" =~ /a/` | after a *failed* match |
|---|---|---|---|
| `defined?($~)` | `"global-variable"` | `"global-variable"` | `"global-variable"` |
| `defined?($&)` | `nil` | `"global-variable"` | `nil` |
| `` defined?($`) `` | `nil` | `"global-variable"` | `nil` |
| `defined?($1)` | `nil` | `nil` — no group | `nil` |

and `defined?($1)` is `"global-variable"` only where **group 1 actually
matched**: `"a" =~ /(a)(b)?/` leaves `defined?($2)` nil, and `$+` — the last
group that participated — answers `"a"`, not `nil`. The rule is *this ref has a
value*, not *the pattern has that many groups*.

`$~ = nil` puts every derived global back to `nil`, and `$~ = 5` raises
`TypeError: wrong argument type Integer (expected MatchData)`.

### A multiple assignment evaluates its targets' receivers **before** the right-hand side

This is the measurement that shaped #191. Ruby does not walk targets after
producing the value:

```
r[(o<<:i1; 0)], r[(o<<:i2; 1)] = (o<<:rhs; 5), 6
#=> [:i1, :i2, :rhs, [:set, 0, 5], [:set, 1, 6]]
```

Both indices, left to right, then the right-hand side, then the writes. The
existing `multi_assign` evaluates the right-hand side first and then walks the
targets, which is indistinguishable for a local and wrong for an index. The
compound forms have their own order, also measured:

```
r[(o<<:idx; 0)] += (o<<:rhs; 2)   #=> [:idx, [:get, 0], :rhs, [:set, 0, 3]]
o.b ||= 2   # when o.b is truthy #=> [:bget]            — the setter never runs
o[0] ||= 2  # when o[0] is truthy#=> [[:get, 0]]        — likewise
```

And the value of the expression is the right-hand side, never the setter's:
with `def []=(k, v); ...; :setter_return; end`, `(c[0] = 5)` is `5` and
`(c[0] += 2)` is `3`.

### `alias` copies; `undef` is not `remove_method`

```ruby
c = Class.new { def a; :first; end; alias b a; def a; :second; end }
[c.new.b, c.new.a]        #=> [:first, :second]
```

The alias holds the body the method had *at the point of aliasing*. And the
distinction the issue asks to pin:

```ruby
sup = Class.new { def a; :sup; end }
Class.new(sup) { def a; :sub; end; remove_method :a }.new.a  #=> :sup
Class.new(sup) { undef a }.new.a                             #=> NoMethodError
```

`undef` also makes `respond_to?` and `method_defined?` answer `false`, while a
subclass defining `a` again is reached normally. `alias` keeps the original's
visibility (a private method aliases to a private method), aliasing a name
nothing defines is a `NameError`, and both `alias` and `undef` evaluate to
`nil`. `alias_method` returns the new name as a symbol.

### Interpolating a `Regexp` embeds its options

```ruby
b = /\d+/  ; /a#{b}c/.source   #=> "a(?-mix:\\d+)c"
b = /\d+/i ; /a#{b}c/.source   #=> "a(?i-mx:\\d+)c"
```

That is `Regexp#to_s`, which is what ordinary interpolation already sends. The
lowering therefore needs no special case for a regexp operand — it needs
`Regexp#to_s` to be right, and a `Regexp.new` that takes the built string.

### A subclass of a built-in keeps its own class and gets the built-in's shape

```ruby
k = Class.new(String); s = k.new("abc")
[s.class == k, s.size, s == "abc", (s + "d").class]  #=> [true, 3, true, String]
k = Class.new(Proc); k.new                           #=> ArgumentError
```

So the subclass answers its own class, the primitives accept it, and a class
with no buildable representation still refuses — including through a subclass,
which is the wrong answer #199 exists to remove.

## Decisions

### #197 — the engine half exists; this is three compiler arms and one instruction

`MatchRef` and `Insn::LastMatch` have been there since #14 with a `Group(u16)`
variant nothing emitted. Reading `$1` and `$&` is routing
`VarRef::NumberedRef`/`VarRef::BackRef` into it — `$&`, `` $` `` and `$'`
already have variants, and `$+` gains `MatchRef::LastGroup`, whose rule is *the
last group that participated*, measured above.

`defined?` cannot reuse that: `Insn::LastMatch` pushes the value, and a group
that matched the empty string is a value `defined?` must call defined while
`nil` is not. So `Insn::DefinedMatch(MatchRef)` asks the same question and
answers the string or `nil`. `$~` keeps its constant `"global-variable"`.

Assigning `$~` becomes `Insn::SetLastMatch`, which type-checks its operand —
`nil` or a `MatchData`, `TypeError` otherwise — and writes the heap's last
match, so `$1` and `$&` follow it. `$~` stays one `Value` per heap, as
`Heap::last_match`'s existing `ponytail:` note says; this slice does not widen
that and does not make it worse.

### #191 — a target is *prepared*, then read and written

`Slot` is the compiler's abstraction for "somewhere `emit_get` and `emit_set`
can reach without touching the stack". Local, ivar and global all satisfy that
for free. An index or attribute target does not: its receiver and its index
arguments have to be evaluated exactly once, before either the read or the
write, and both of those are sends.

`Slot` gains a `Send` variant holding the hidden locals the receiver and the
arguments were parked in, plus the getter and setter names. `target_slot`
becomes a **preparing** operation: for a `Send` target it emits the receiver
and argument evaluation *now*, into those locals, and returns a slot that can
be read and written any number of times afterwards without re-evaluating them.
Every existing caller already invokes it before evaluating the right-hand side,
so the compound forms get Ruby's order without moving anything.

Multiple assignment does have to move: `multi_assign` gains a walk that
prepares every target before it evaluates the right-hand side, which is the
order measured above. Preparation is a no-op for the local, ivar and global
targets that make up almost every multiple assignment in the corpus, so the
only assignments that gain instructions are the ones that were refused.

Hidden locals are named `%idx{offset}` and are per site, so
`a[i] = (b[j] = 1)` gets two sets of them rather than one clobbered set — the
same reasoning, and the same naming trick, that `%attr` already uses for
`a.b = v`.

`a&.b ||= v` stays refused, under the existing safe-navigation reason, and a
block on an index target (`a[&b] ||= 1`) is refused by name rather than
dropped.

### #188 — one primitive, three spellings

`alias new old`, `Module#alias_method` and `Module#undef_method` are the same
two operations on a method table. The table lives in `Classes`, so the
primitives are `Native::AliasMethod` and `Native::UndefMethod`, and
`core/module.rb` spells the two public methods on top of them. The `alias`
statement is not a send — it works on the frame's **definee**, the same cref
`def` writes into, not on `self` — so it compiles to `Insn::Alias`, which reads
the definee out of the frame exactly as `Insn::DefineMethod` does.

`undef` installs a tombstone rather than removing the entry, because a removal
would let the superclass's method through and Ruby does not:
`Method::Undefined` is a body a lookup finds and then reports as *not found*.
`respond_to?`, `method_defined?` and `NoMethodError` all fall out of that one
representation, and a later `def` of the same name overwrites the tombstone.

`alias $new $old` is a **true alias** — writing the new name changes the old
one, measured — which the name-keyed global table cannot express without a
level of indirection. One file in the whole corpus uses the form, so it is
refused by its own name (`` `alias` on a global variable ``) rather than
answered wrongly or bundled back into #188's reason.

### #189 — build the source, then `Regexp.new` it

An `Iseq` literal is immutable and shared across Ractors, so an interpolated
pattern cannot be one. The lowering is the concatenation #154 already built for
strings, followed by `Regexp.new(source, options)` — which means the options
have to survive as a value, so `regexp_options` is emitted as an integer
argument rather than folded into a literal.

`Regexp.new` gains a `String` operand. `spinel-regex` already compiles a pattern
from a source string, so this is the existing primitive with its argument
arriving at run time instead of compile time.

`/o` — interpolate once and cache — is refused **by its own name**, because a
lowering that ignores it would re-interpolate and answer a different pattern the
second time through. Measured, and it is stronger than "cache the source": the
literal site caches the `Regexp` *object*.

```ruby
def f(b) = /#{b}/o
a = f(1); b = f(2)
[a.equal?(b), a.source]   #=> [true, "1"]
r = []; 2.times { |i| b = i; r << /#{b}/o.source }
r                         #=> ["0", "0"]   ("0", "1" without /o)
```

That is a per-site cache slot in the `Iseq`, which is a different thing from
this slice's lowering and is why the flag is refused under its own reason rather
than folded into "regexp interpolation".

### #199 — the question a primitive asks about its receiver

`Builtin::ALL` is indexed by class id, so a class the bootstrap did not create
is `None` for its own id and `allocate_instance` hands back a plain object. The
issue records that routing such a class to its nearest built-in ancestor's
*class object* costs 45 examples, because `MyString.new.class` then answers
`String`.

So the fix is not in the allocation arm, it is in the **question**: an `Entry`
gains `repr: Option<Builtin>`, the representation instances of this class have,
inherited from the superclass when the class is created and equal to the
built-in itself for a built-in. `allocate_instance` then allocates the shape
`repr` names while wearing the *actual* class, and `heap_kind` — which runs on
every `==` — asks `repr` instead of comparing two class objects, which is one
table read where it was two comparisons.

A class whose `repr` has no buildable representation still refuses, so
`ProcSubclass.new` raises where `Proc.new` raises rather than answering an
object. `bench/method_cache.rs` is the check that the receiver question did not
get more expensive.

### #192 — closed in favour of #145, and the reasoning recorded

The 310 examples are real and this slice does not take them.

The examples are built by a loop whose block parameters the harness never
binds, because it walks the `each` block looking for `it` blocks rather than
running it. Taking them means evaluating the iterating expression and cloning
the examples inside once per element — a real evaluator inside `discover.rs`,
which is the thing `spec/harness` exists to avoid, and the same machinery
`it_behaves_like` and `evaluate` would each need.

`spec/harness` is deleted at the end of phase 2, when mspec runs on Spinel
(#145). mspec runs the loop because it runs the file. So the work is an
evaluator, in the component scheduled for deletion, for a limitation that
disappears when the component does — and 231 of the 310 are in
`library/socket/`, which needs `Socket` before any of them could pass anyway.

The rule #164 established stays: `spec/harness` does not invent a value for a
name it did not bind. Refusing remains the correct answer, and the refusal keeps
naming the enclosing scope so the count stays visible in the ranking.

## Tasks

- [x] Measure every rule above against ruby 4.0.6, one fresh process per
      `$~` question
- [x] #197 — `MatchRef::LastGroup`; `VarRef::BackRef`/`NumberedRef` read;
      `Insn::DefinedMatch`; `Insn::SetLastMatch` with its `TypeError`
- [x] #191 — `Slot::Send`, preparing `target_slot`, target preparation before
      the right-hand side in `multi_assign`
- [x] #188 — `Native::AliasMethod`/`UndefMethod`, `Method::Undefined`,
      `Insn::Alias`, `core/module.rb` spellings
- [x] #189 — run-time regexp source, `Regexp.new(String, options)`, `/o` refused
      by name
- [x] #199 — `Entry::repr`, allocation by representation, `heap_kind` by
      representation, the two tags deleted
- [x] #192 — closed with the reasoning above
- [x] `crates/spinel-vm/tests/eval.txt` rows for every measured rule, checked by
      `scripts/eval-oracle.rb --check`
- [x] `scripts/regexp-oracle.rb` still agrees
- [x] `scripts/verify-passes.rb` agrees on every claimed pass
- [x] Spec delta per directory, stated below

## Result

```
before  3835 files · 25624 examples · 1823 passed · 0 failed · 21890 blocked · 1911 skipped
after   3835 files · 25624 examples · 2051 passed · 0 failed · 21653 blocked · 1920 skipped
```

**+228 passing, and the failed column stayed at zero.** Per directory:

| | before | after | |
|---|---|---|---|
| `language/` | 1097 | **1220** | +123 |
| `core/array/` | 142 | **169** | +27 |
| `core/kernel/` | 87 | **117** | +30 |
| `core/module/` | 79 | **92** | +13 |
| `core/string/` | 43 | **51** | +8 |
| `core/regexp/` | 27 | **33** | +6 |
| `core/hash/` | 39 | **44** | +5 |
| `core/proc/` | 19 | **18** | −1, deliberately — see below |

Every refusal this PRD names is gone from the ranking:

```
  45  `alias` or `undef` is not compiled yet                     → 0
  83  an index assignment is not compiled yet                    → 0
  35  an attribute assignment is not compiled yet                → 0
  27  regexp interpolation is not compiled yet                   → 0
  22  `defined?` on a back-reference is not compiled yet         → 0
  15  a regexp back-reference is not compiled yet                → 0
  15  assigning a regexp special variable is not compiled yet    → 0
   8  a class variable is not compiled yet                       → 0
```

`scripts/verify-passes.rb` re-ran the 1220 claimed passes it covers on ruby
4.0.6: all agree. `scripts/eval-oracle.rb --check` and
`scripts/regexp-oracle.rb --check` both agree. `bench/method_cache.rs` is
unchanged where #199 touched it — a cached lookup is 8.0ns and an inline-cache
hit 3.0ns on both sides, because asking a class for its representation is one
table read where comparing two class objects was two comparisons. Boot moved
100.2µs → 104.6µs per heap, which is the extra `core/*.rb` rather than the
receiver question.

### The one lost pass was not earned

`core/proc/new_spec.rb`'s *"calls initialize on the Proc object"* passed before
this branch and is blocked after it. `ProcSpecs::MyProc2.new(:a, 2) { }` is the
exact bug #199 was filed about: the subclass got the plain-object shape, so
`initialize` ran and set `@first` and `@second`, and the example checked those
two ivars and nothing else. The object was not a `Proc` — calling it would have
read past its end. It now refuses, along with `Proc.new` itself, which is the
honest answer and what #199's third bullet asks for. A pass that measured a
wrong answer became a blocked example that measures a missing one.

### Class variables were needed, and were not in any issue

#188's definition of done requires `core/module/fixtures/classes.rb` and
`core/kernel/fixtures/classes.rb` to compile. Both use `@@a` at class-body
level, so `alias` and `undef` alone left them refused one line further down, and
no open issue covered it. So this slice implements them: a per-class table
beside the constants, `Insn::GetCvar`/`SetCvar`/`DefinedCvar` reading the
frame's cref the way `def` does, and `Module#class_variables`,
`#class_variable_get`, `#class_variable_set` and `#class_variable_defined?` over
the same table.

The rules were measured, and two are not what reading the code suggests:

- A **write** lands on the ancestor that already holds the name, so a subclass
  assigning `@@a` changes the superclass's.
- `@@a ||= 1` on an unset one is `1`, while `@@a &&= 1` and `@@a += 1` are both
  `NameError`. Only `||=` is guarded, so the compiler guards exactly that one.
- Two classes in a chain holding the same name is Ruby's `RuntimeError: class
  variable @@a of Sub is overtaken by Sup` rather than a silent nearest-wins.
- A class variable at the top level is `RuntimeError: class variable access from
  toplevel`, not a write to `Object`.

### What the fixtures still wait on

The compiler half of #188 and #189 is done — all four fixture files compile —
but two of them now fail at *load* on missing methods rather than on syntax:

| fixture | stops at | belongs to |
|---|---|---|
| `language/fixtures/classes.rb` | — loads clean | ✅ `ClassSpecs` 46 → 26 blocked |
| `language/fixtures/send.rb` | a destructuring block parameter | #11's `ParamSpec` |
| `core/module/fixtures/classes.rb` | `Module#private_constant` | #185 |
| `core/kernel/fixtures/classes.rb` | `Kernel#abort`, `exit`, `fork`, `system` | phase 3 |

`ModuleSpecs` fell 219 → 179 and `KernelSpecs` 185 → 126 on the strength of the
part that does load. The rest is named above rather than left in the ranking as
"uninitialized constant".

### Failures this branch revealed, and where they went

Unblocking four fixture files ran several thousand examples for the first time,
and an example that has never run cannot have been disagreeing with Ruby yet.
Twenty-one appeared. Ten were small enough to fix in the session that revealed
them, which is what `spec/tags/README.md` asks for first:

- `Exception#message` and `#inspect` go through `to_s`, so a subclass that
  overrides it is seen — 3
- `Kernel#is_a?` and `#instance_of?` raise `TypeError` on a non-module, on the
  false path only so the common path costs what it did — 2
- `Regexp#initialize` on an already-built pattern is `TypeError`, and
  `FrozenError` on a literal — 2
- `Array#concat` and `#shift` check frozen — 2
- `Hash#[]` misses through `default`, which is a method a subclass overrides,
  rather than reading `@default` — 1

The remaining eleven are other subsystems. Each is filed and tagged, with the
issue named in the tag:

| | | |
|---|---|---|
| #201 | `dup`/`clone` never send `initialize_copy` | 3 |
| #202 | `is_a?` asks `self.class`, missing `extend` and honouring an override | 2 |
| #203 | `String#match` does not dispatch to `Regexp#match` | 1 |
| #204 | `return` in a `class << obj` body | 1 |
| #205 | spinel-regex accepts backreferences Ruby rejects | 2 |
| #207 | `instance_variable_set` name errors are `TypeError` | 1 |
| — | `optional/capi/` needs the C-API, which Spinel will not have | 1 |

#206 was filed separately and is not one of the eleven: `$!` is never set when
an exception is rescued, found while writing tests against this branch. It is
quiet in a way worth fixing early — every `x rescue $!.class` in a test reads
`nil` and looks like it passed a different way.

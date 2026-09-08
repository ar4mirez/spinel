# PRD 0030 — Constant visibility, `$!`, and the destructured block parameter

Tracks [#185](https://github.com/ar4mirez/spinel/issues/185),
[#206](https://github.com/ar4mirez/spinel/issues/206), and
[#209](https://github.com/ar4mirez/spinel/issues/209). Milestone: Phase 1: a VM that runs
`language/`. `P1`, `size:S` each, `area:engine`.

## Objective

Three defects that #208 left behind, none of which needs a new subsystem. Each is a rule
the VM already has the machinery for and does not apply.

Measured on the branch point (`d4d2d1f`), whole corpus:

```
3835 files · 25624 examples · 2051 passed · 0 failed · 21653 blocked · 1920 skipped
```

### 1. Constant visibility (#185)

Two lookup rules are missing, and both surfaced as *failures* rather than blocks when #183
taught `spec/harness` to load ruby/spec fixtures.

`Module#private_constant` does not exist at all — not "accepted and does nothing", as the
issue reads. The consequence is worse than a no-op, because the harness loads a fixture
leniently: `spec/ruby/language/fixtures/constant_visibility.rb` defines
`ConstantVisibility::ModuleContainer::PrivateModule`, then raises `NoMethodError` on the
`private_constant` line and abandons the rest of the file. So the constant exists and is
not marked, the two "cannot be reopened" examples reopen it and see no error, and the 18
examples that need the *rest* of that fixture — `PrivConstModule`, `PrivConstClass`,
`ClassIncludingPrivConstModule`, `PrivConstClassChild` — are reported blocked on a
constant that a working `private_constant` would have defined.

`BasicObject` has no empty constant namespace. `Classes::const_get`'s last step is
"a class already reached `Object` above; a module did not", spelled as
`if ancestors.contains(&object) { return None }`. That is the right rule for every class
whose chain includes `Object` and the wrong one for the two that do not: `BasicObject`
itself and anything below it. CRuby's own condition is not about the ancestors at all —
`rb_const_search` retries against `rb_cObject` only when `BUILTIN_TYPE(klass) == T_MODULE`
— so the fallback belongs to *being a module*, and a `class Foo < BasicObject` body is
simply not one.

### 2. `$!` (#206)

`p (raise("x") rescue $!.class)` answers `NilClass`. #166 gave the language a global table
and `$~` its own instruction; `$!` was wired to neither, so it reads out of the ordinary
global table, which nothing writes.

Measured on ruby 4.0.6, `$!` is not frame-local — a method or block called from inside a
`rescue` sees it — and it is restored on *every* path out of the clause, including
`return`, `break`, and a re-raise that an outer clause catches:

| written | `$!` |
|---|---|
| `def show; $!.inspect; end; begin; raise "x"; rescue; show; end` | `#<RuntimeError: x>` |
| `begin; raise "a"; rescue; (raise "b" rescue nil); $!.message; end` | `"a"` |
| `begin; begin; raise "u"; ensure; $!; end; rescue; end` (in the `ensure`) | `#<RuntimeError: u>` |
| after any of the above completes | `nil` |
| `begin; 1; rescue; 2; else; $!; end` | `nil` |
| `defined?($!)`, inside a clause or outside one | `"global-variable"` |

So it is one per-heap cell beside `last_match`, saved on entering a protected region and
restored on leaving it, and set by the unwinder when a handler takes an exception.

### 3. The destructured block parameter (#209)

`{ |(a, b), c| }` is refused, because `ParamSpec` carries a *count* of required parameters
and the binder writes the n-th argument to slot n. A destructure binds several names out
of one argument, so every parameter after it is displaced by however many names the
destructure introduced, and `Compiler::spec_from_list` refuses the shape rather than
binding `c`'s argument into `b`'s slot.

The direct count is seven examples. The leverage is elsewhere:
`spec/ruby/language/fixtures/send.rb` refuses on exactly this, so `LangSendSpecs` is never
defined and **71 of `send_spec.rb`'s 78 examples** are blocked on
`NameError: uninitialized constant LangSendSpecs`.

## Non-goals

- **`public_constant`, `deprecate_constant`, `const_defined?`, `constants`,
  `remove_const`.** Reflection is [#28](https://github.com/ar4mirez/spinel/issues/28)'s.
  This slice adds `private_constant` because a *lookup rule* needs somewhere to read
  visibility from, and adds nothing else that only reads it back.
- **`const_missing`.** `private_constant_spec.rb` asserts that a blocked private constant
  sends `const_missing` to the module. That hook is #28's too; until it exists, the
  qualified reference raises the `NameError` Ruby raises when no hook is defined, which is
  the answer for every module in the corpus that does not define one.
- **`$!` as a raise-time value.** The cell is set where a handler takes the exception —
  the `rescue` and `ensure` arms of the unwinder — not at the `raise`. Every observation
  point in the table above is inside a handler, so the difference is not observable from
  Ruby; a `raise` that nothing catches ends the program.
- **A slot per parameter for anything but required and post.** `optional`, `rest`,
  `keywords`, `kwrest`, and `block` already carry explicit slots. Only the two counts move.
- **A destructure that binds no name at all** (`{ |(*), c| }`). It has no first inner name
  to borrow a slot from. Refused with its own reason rather than mis-bound; see R3.

## Rules

- **R1. `private_constant` records visibility; the lookup enforces it.** A private constant
  is invisible to `A::X` and to `A.const_get`-shaped qualified reference, and fully visible
  to a bare `X` written in the module's own lexical scope or in a scope nested inside it.
  Enforcement lives in `const_get_qualified`, which is the one path a qualified reference
  takes; `const_get`'s lexical walk is left alone, because that walk *is* the scope the
  constant is private to.
- **R2. The fallback to `Object` is for modules.** `Classes::const_get`'s last step runs
  when the innermost lexical scope is a module and never when it is a class. Written as
  the question CRuby asks, not as a property of the ancestor list that happens to agree
  for every class but two.
- **R3. Refuse rather than mis-bind, still.** The reason
  `"a destructuring block parameter before another parameter"` leaves the report. The
  narrower `"a destructuring block parameter that binds no name"` replaces it for
  `{ |(*), c| }`, which is the one shape the borrowed-slot scheme cannot place.
- **R4. `$!` is restored on every path out.** The compiler saves the cell into a hidden
  local before a protected body and restores it after the construct; the frame records the
  cell on entry and the unwinder restores it when the frame is popped, which covers
  `return` and `break` out of a clause without the compiler seeing them.
- **R5. Measured, not reasoned.** Every row this slice adds to
  `crates/spinel-vm/tests/eval.txt` is run against system `ruby` first. The tables in the
  Objective are that measurement.

## Definition of done

- [x] `private_constant` exists on `Module`, takes one or more names as `Symbol` or
      `String`, and returns `self`
- [x] A private constant raises `NameError` on a qualified reference from outside, and
      resolves from the module's own lexical scope
- [x] A bare constant inside `BasicObject` or a subclass of it does not fall back to
      `Object`
- [x] `$!` is the exception inside a `rescue`, `nil` outside one, set by the modifier
      form, and restored when a nested `rescue` finishes
- [x] `{ |(a, b), c| }` binds `c` to the second argument, and the same for a destructure
      before an optional, a rest, a post, a keyword, and a block parameter
- [x] `language/fixtures/send.rb` compiles and `send_spec.rb` no longer blocks on
      `LangSendSpecs`
- [x] `"a destructuring block parameter before another parameter"` leaves
      `scripts/spec.sh --blocked=0`
- [x] The four tags in `spec/tags/language/constants_tags.txt` and
      `spec/tags/core/basicobject/basicobject_tags.txt` are deleted rather than reworded
- [x] Rows in `crates/spinel-vm/tests/eval.txt` for all three, measured against CRuby
- [x] ruby/spec delta named in the PR, per directory

## Result

Measured over the whole corpus, `d4d2d1f` against this branch, same ruby/spec checkout:

```
base  3835 files · 25624 examples · 2051 passed · 0 failed · 21653 blocked · 1920 skipped
head  3835 files · 25624 examples · 2085 passed · 0 failed · 21623 blocked · 1916 skipped
```

**+34 passes, 0 failures, 4 fewer skips** — the four deleted tags. Per directory, and the
per-directory numbers sum to the aggregate:

| directory | base | head | |
|---|---:|---:|---:|
| `language` | 1220 | 1243 | +23 |
| `core/module` | 92 | 102 | +10 |
| `core/basicobject` | 6 | 7 | +1 |

Per file:

| file | base | head | | why |
|---|---:|---:|---:|---|
| `language/constants_spec.rb` | 33 | 50 | +17 | #185, both halves |
| `language/block_spec.rb` | 91 | 96 | +5 | #209 |
| `language/yield_spec.rb` | 33 | 34 | +1 | #209 |
| `core/module/private_class_method_spec.rb` | 0 | 3 | +3 | its fixture calls `private_constant` |
| `core/module/public_class_method_spec.rb` | 0 | 3 | +3 | the same fixture |
| `core/module/protected_spec.rb` | 0 | 2 | +2 | the same fixture |
| `core/module/remove_class_variable_spec.rb` | 3 | 4 | +1 | the same fixture |
| `core/module/to_s_spec.rb` | 1 | 2 | +1 | the same fixture |
| `core/basicobject/basicobject_spec.rb` | 1 | 2 | +1 | #185 |

`scripts/verify-passes.rb` re-ran all 1,352 claimed passes in those three directories on
ruby 4.0.6: all agree. 57 new rows in `crates/spinel-vm/tests/eval.txt`, every one measured
first.

### What the numbers do not say

- **#209's 71 examples are not in the +34.** `language/fixtures/send.rb` compiles now and
  `LangSendSpecs` is defined, so the reason `NameError: uninitialized constant
  LangSendSpecs` is gone from the report — the DoD as written. The 71 examples behind it
  did not start passing: they moved onto `Module#module_function` with no arguments, which
  sets the mode for the rest of the body and is a second piece of scope state plus a branch
  in `Insn::DefineMethod`. That is a separate slice, and the issue's "71 examples behind
  it" was one construct short.
- **One `defined?` example moved from failed to blocked, not to passed.**
  `BasicObject does not define built-in constants (according to defined?)` used to answer
  `"constant"`, which was wrong. It now reaches `Error::Unknowable` — `defined?` of a name
  this heap has never seen cannot be answered before `require` (#39) — along with 303 other
  examples. The tag goes because a tag is for an example Spinel *runs and gets wrong*, and
  this one is now honestly blocked.
- **Two constant-precedence answers changed that no issue asked about.** Skipping the
  outermost cref node is what fixes `BasicObject`, and the same change makes an ancestor's
  constant beat a top-level one of the same name. Spinel answered 2 where Ruby answers 1
  for both `class C < P; X; end` and `class D; include M; Y; end`; measured and now in
  `eval.txt`.

## Tasks

1. **`$!`** — `Heap::errinfo` beside `last_match`; `Insn::Errinfo` and `Insn::SetErrinfo`;
   route `VarRef::Global("$!")` to them in read, write, and `defined?` position; save and
   restore around a `begin` that has a `rescue` or an `ensure`; set the cell in both arms
   of the unwinder's catch; restore from `Call::errinfo_on_entry` when a frame is popped.
2. **Slot per parameter** — `ParamSpec::required` and `ParamSpec::post` become
   `Vec<u16>`; `spec_from_list` claims a slot per inner name of a destructure and advances
   the cursor by that many; `bind` and `spreads` read the slots; `Zsuper::from_spec` drops
   its `first_post` arithmetic; `emit_defaults` reads the recorded slot.
3. **Constant visibility** — a per-module set of private constant names in `ClassEntry`;
   `const_get_qualified` skips them; `Module#private_constant` as a native; the `Object`
   fallback asks whether the innermost scope is a module.
4. **Measure** — `eval.txt` rows for each, run against system `ruby`; delete the four
   tags; per-directory spec delta for the PR.

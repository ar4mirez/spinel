# PRD 0031 — Bare `module_function`, the `for` loop, and a splat as an exit value

Tracks [#211](https://github.com/ar4mirez/spinel/issues/211),
[#212](https://github.com/ar4mirez/spinel/issues/212), and
[#213](https://github.com/ar4mirez/spinel/issues/213). Milestone: Phase 1: a VM that runs
`language/`. `P1`, `size:S` each, `area:engine`.

## Objective

Three constructs the VM refuses, each of which has its machinery already built and applies
it one shape short. None needs a new subsystem, and none needs a new opcode.

Measured on the branch point (`1122f29`), whole corpus:

```
3835 files · 25624 examples · 2085 passed · 0 failed · 21623 blocked · 1916 skipped
```

The three reasons, from `scripts/spec.sh --blocked=0`:

```
30  a `for` loop is not compiled yet
17  a splat is not compiled yet
14  `Module#module_function` on no arguments, which sets the mode for the rest of the body
```

Behind the third sit `send_spec.rb`'s 71 blocked examples, which #209 was filed to reach
and did not: it cleared the construct it named, and
`spec/ruby/language/fixtures/send.rb` then stopped one line further down, on the bare
`module_function` that opens it.

### 1. Bare `module_function` (#211)

`Native::ModuleFunction` implements the *argument* form and refuses the bare one, because
the bare form is a mode rather than an operation: it changes what every `def` below it in
the same body means.

#161 already put that kind of state on the frame as `Frame::scope_visibility`, set by a
bare `private`/`public`/`protected` and read by `Insn::DefineMethod`. `module_function` is
the same mechanism with a different effect on definition, so the two share one field
rather than becoming two that can disagree — a body is in exactly one default.

Measured on ruby 4.0.6 (`scripts/module-function-oracle.rb`):

| written | result |
|---|---|
| `module_function` then `def a` | `a` is a public singleton method **and** a private instance method |
| `module_function`, then `private`, then `def b` | `b` is private-instance only — `private` **replaces** the mode |
| `private`, then `module_function`, then `def c` | `c` is a module function — the mode replaces again |
| `module_function`; `[1].each { def m; end }` | `m` is a module function — a block shares the mode |
| `module_function`; `def outer; def nested; end; end` | `nested` is a plain public instance method — a method body resets it |
| `module_function`; `def self.direct; end` | unaffected; already a singleton method |

The last two are exactly `private`'s measured behaviour under #161, which is the point:
one field, one rule about where it lives.

### 2. A `for` loop (#212)

`for` is the last control-flow keyword the compiler refuses, and the milestone is "a VM
that runs `language/`", of which `language/for_spec.rb` is 29 of the 30 blocked examples.

`for x in xs` is not `xs.each { |x| }`, because `for` does not open a scope. Measured on
ruby 4.0.6:

| written | result |
|---|---|
| `for i in [1,2,3]; end; i` | `3` — the variable outlives the loop |
| `for e in []; end; defined?(e), e` | `"local-variable"`, `nil` — bound even when nothing ran |
| `for a in xs; end` | answers `xs`, the collection itself |
| `for a, b in [[1,2],[3,4]]; end; [a,b]` | `[3,4]` — destructures like a block parameter |
| `for k in [1] do break 9 end` | `9` |
| `for k in [1,2,3]; next if k == 2; end` | skips the iteration |

So the lowering is `each` with a block whose body writes the *enclosing* local through
`SetLocal(slot, depth)` rather than binding a parameter — the calling convention #11 built.
`break` and `next` then mean what they mean in a block, which the unwinder already does.

### 3. A splat as the value of `return`, `break`, or `next` (#213)

The reason reads "a splat", which is misleading: a splat in a call, an array literal and a
multiple assignment all compile. The 17 are one shape — a splat as the whole value of a
non-local exit.

`jump_value` in the parser already normalises `return *a, b` into an `Array` literal, which
compiles today. It leaves a lone `return *ary` as a bare `ExprKind::Splat`, which nothing
below it expects. Measured on ruby 4.0.6:

| written | answers |
|---|---|
| `ary = []; return *ary` | `[]` |
| `ary = [1]; return *ary` | `[1]` — **not** `1`; there is no one-element unwrap |
| `ary = [1, 2]; return *ary` | `[1, 2]` |
| `value = 1; return *value` | `[1]` |
| `nil_value = nil; return *nil_value` | `[]` |
| `obj` with `def obj.to_a; [1, 2]; end` | `[1, 2]` |

That is `[*x]` — the array literal's `Array#__concat_splat__` rule exactly, including the
`to_a` conversion and the `nil` case. So the lone splat wants the same normalisation the
multi-value case already gets, not a rule of its own.

## Tasks

- [x] T1 `ScopeDefault` replaces `Frame::scope_visibility`, carrying the fourth mode
- [x] T2 Bare `module_function` sets it; `Insn::DefineMethod` branches on it
- [x] T3 `for` lowers to `each` with an enclosing-scope-writing block
- [x] T4 `for`'s target destructures, and the loop answers the collection
- [x] T5 `jump_value` normalises a lone splat to a one-element array literal
- [x] T6 Oracle scripts for each rule above, generated from CRuby
- [x] T7 Rows in `crates/spinel-vm/tests/eval.txt`, measured against CRuby
- [x] T8 `language/fixtures/send.rb` runs to the end (memory: a fixture unblock is often
      one construct short — run the fixture, do not infer from the reason)
- [x] T9 The three reasons leave `scripts/spec.sh --blocked=0`
- [x] T10 ruby/spec delta named, per-directory, via worktree

## Definition of done

Per issue, as filed. Plus: `cargo test` green in debug and release, and no reason that
this work introduced appears in the corpus ranking.

## Results

Whole corpus, `1122f29` → this branch:

```
before  3835 files · 25624 examples · 2085 passed · 0 failed · 21623 blocked · 1916 skipped
after   3835 files · 25624 examples · 2150 passed · 0 failed · 21558 blocked · 1916 skipped
```

`+65`, no failures. The per-directory and per-file deltas both sum to `+65`, and no file
moved down — the check that says a delta is real rather than a reshuffle.

| file | before | after |
|---|---|---|
| `language/send_spec.rb` | 7 | 41 (+34) |
| `language/for_spec.rb` | 0 | 18 (+18) |
| `language/return_spec.rb` | 18 | 23 (+5) |
| `language/break_spec.rb` | 28 | 32 (+4) |
| `core/module/module_function_spec.rb` | 2 | 6 (+4) |

By directory: `language` 1243 → 1304, `core/module` 102 → 106.

All three reasons left `scripts/spec.sh --blocked=0`. `scripts/verify-passes.rb` re-ran all
1304 `language/` passes on ruby 4.0.6: all agree, so none of the new passes is false.

### What the work found that the issues did not predict

**`Hash#each` yielded two values where CRuby yields one.** `for k, v in hash` is in
`for_spec.rb`, and it bound `v` to `nil`. The cause was in `core/hash.rb`, not in `for`:
it yielded `pair[0], pair[1]`, so a `{ |x| }` block over a hash bound `x` to the key alone
where CRuby binds it to `[key, value]`. Measured — `{a: 1}.each { |*a| }` receives one
argument — and fixed to `yield pair`; the two-local shape comes from the block's own
auto-splat. Pre-existing and unrelated to the three issues, surfaced by the first of them
to iterate a hash.

**"A splat is not compiled yet" was two constructs.** 12 of the 17 were the exit value;
the other 5 are `rescue *classes`, which is a run-time number of `CheckMatch`es and not
this slice. Refused under its own name — "a splat in a `rescue` list" — so the generic
reason leaves the ranking and what remains says which construct it is. Filed as follow-up.

**A `for` body's environment is not a Ruby scope.** `for_spec.rb` has an explicit example
for implementations that give a `for` body its own run-time scope, which this lowering
does. It nests a `for` inside a block inside a `for`, and it only works if Prism's depths
are translated rather than used: `real_depth` walks the chain and skips the environments
Ruby cannot see. Verified against CRuby by hand as well as by the spec, because the
spec example itself is still blocked on `local_variables`.

### Left behind, deliberately

- `rescue *classes` (5 examples), named above.
- `core/hash.rb`'s `inspect` writes `{:b => 10}` where ruby 4.0.6 writes `{b: 10}` for
  symbol keys. Pre-existing, unrelated, and not in these issues' scope.

# PRD 0024 — `spec/harness`: an example sees the locals of the scopes enclosing it

Issue: [#164](https://github.com/ar4mirez/spinel/issues/164) · Phase 1 · `area:engine`

## Objective

`spec/harness` flattened an `it` into one scope holding the block's own locals
plus the locals of every `before` it inherits, and nothing else. A local declared
in a scope that *encloses* the example — the spec file's top level, or a
`describe` body — was in neither list, so the example was reported

```
blocked: a local variable from an enclosing scope is not compiled yet
```

which names the compiler. The compiler was never the problem:

```ruby
x = 1
[1].each { |a| [2].each { |b| [3].each { puts x } } }   # Spinel: 1
```

`Compiler::nested` chains `parent.locals` and `parent.outer`, so a block already
sees every enclosing block scope. What was missing is that the harness never
*told* it about the scopes above the example.

## Baseline

Measured on `main` at 098cb63, before any change.

| | |
|---|---|
| Rust tests | 254 passing · 0 failing |
| ruby/spec corpus | 25,624 examples · 1,564 passed · 0 failed · 1,875 skipped |
| `language/` | 2,735 examples · 890 passed · 0 failed |
| blocked on an enclosing local | 406 corpus · 50 `language/` |
| `verify-passes.rb` | 890 re-run on ruby 4.0.6, all agree |

The issue was filed at 133 corpus examples and re-triaged at 338. It was 406 by
the time this slice started, which is the third time that number has grown while
the issue sat — and it is `size:S`.

## Decisions

### The premise was checked before it was built on

`spec/tags/README.md` and this project's memory both say a blocked reason can
name the wrong subsystem. The reason here named `compile.rs`, so the first thing
this slice did was run the shape as a plain `.rb` file through `spinel run`. It
printed `1`. The compiler was fine and the harness was not, exactly as the issue
claimed — but the claim was worth ten seconds to falsify rather than assume.

### Flattening is the chosen model, stated rather than inherited

Three scopes, one variable, and the `before` writes *through* to the
`describe`'s slot rather than shadowing:

```ruby
describe "Comparable#==" do
  a = b = nil                                  # declared here
  before(:each) { a = ComparableSpecs::Weird.new(0) }   # assigned here
  it "..." do (a == a).should == true end      # read here
end
```

Flattening all three into one scope gives the right answer, but only because the
write and the read agree on the name. What it cannot model is a name
deliberately shadowed at two depths, and nothing in the corpus does that. The
`Walk::scope` doc comment says so, so the next reader finds a decision rather
than an accident.

### A call taking a literal block is never a scope statement

The first rule tried was precise: prepend everything except the DSL and anything
whose block builds examples. It lost 42 passing examples. 28 of them were
`proc_spec.rb`, whose `describe` body holds

```ruby
evaluate <<-ruby do
  @p = proc { |**nil| :ok }
  ruby
  ...
end
```

`evaluate` is mspec building an example out of a heredoc. Running it reached
`**` at the VM and blocked all 38 examples in the file.

The rule that replaced it is cruder and safer: a call taking a literal block is
structure the walk descends into, never Ruby to run. Every shape that would be
wrong to run takes one — `describe`, `before`, the guards, `evaluate`, a loop
generating examples — and the named list shrinks to the DSL that appears
*without* a block: `it "is pending"`, `it_behaves_like`, and
`require`/`require_relative`, which the loader has already run (#183). The cost
is a group-level `each` that really was setup, which nothing in the corpus needs
today.

### The compiler learns a flattened mode, opt-in and resolve-only

Prism resolves a read against the scopes as written, so a name reaching the
harness carries a `depth` counting scopes the harness has merged.
`compile::flattened_expression` resolves such a depth against the outermost
scope that exists; `compile::expression` still refuses, because for ordinary
Ruby that depth is a disagreement between Prism and this compiler about what a
local is — a bug to hear about, not a shape to lower.

Two things about it were found by being wrong first:

- **The depth is rewritten, not just the slot.** `outer_slot` returned a slot
  while both call sites emitted `Insn::GetLocal(slot, *depth)` with the original
  depth. A frame one environment short of the depth it is handed does not fail;
  it reads whatever is there, and the corpus run died in `HandleScope::slot` on
  a fixnum.
- **It resolves and never creates.** Creating a slot for an unmerged name binds
  `nil`. `symbols.each do |input, expected| ... end` in
  `core/symbol/inspect_spec.rb` then ran against two invented nils and *failed*
  — and nothing about that guaranteed the failing direction rather than a false
  pass. Those 310 examples stay blocked, correctly: their loop needs a VM to
  expand, which is the `it_behaves_like` category, not this one.

### A scope statement that raises is tolerated, and becomes the reason

`send_spec.rb` opens `specs = LangSendSpecs`, and that fixture does not compile
yet — it reaches an attribute assignment. Blocking on the raise took 11 examples
that never mention `specs` down with it, which is the 261-of-263 mistake #183
already documented in a smaller shape.

So a scope statement is run leniently. What keeps that honest is the pair that
already exists: `verify-passes.rb` replays a pass on real Ruby where the
constant does exist, and a *blocked* example reports the scope error rather than
whatever it hit afterwards. The second half is what keeps the ranking
aggregable — 71 examples naming `LangSendSpecs` instead of 50 distinct nil
receivers, which is the difference between data and noise for choosing the next
slice.

### `run` reports the spans that ran, because leniency made them diverge

`verify-passes.rb` rebuilds an example by concatenating the spans the harness
emits. Once a scope statement could be skipped, a static span list claimed one
ran that had not: `lambda_spec.rb`'s `SpecEvaluate.desc = "for definition"` does
not compile in Spinel, and slicing it into the replay made Ruby raise
`NameError` on four examples that had been agreeing. The oracle caught it, which
is what it is for. `run` now returns the spans that actually executed.

## Plan

1. Verify the premise as a plain `.rb` file. ✅
2. `Walk` stacks a group's plain statements alongside its hooks; they run first. ✅
3. `Example.scope` kept apart from `body`, with its own spans. ✅
4. `compile::flattened_expression`, resolve-only, rewriting slot *and* depth. ✅
5. Lenient scope statements; the scope error becomes the blocked reason. ✅
6. `run` reports the spans that ran; `verify-passes.rb` is unchanged. ✅
7. Unit tests for both shapes and for each rule found by being wrong. ✅

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| corpus passed | 1,564 | **1,577** |
| corpus failed | 0 | **0** |
| `language/` passed | 890 | **900** |
| blocked on an enclosing local (corpus) | 406 | **310** |
| blocked on an enclosing local (`language/`) | 50 | **0** |
| `verify-passes.rb` | 890 agree | **900 agree** |

No example that passed before stopped passing.

The 310 that remain are one shape and not this one: a block parameter of a loop
that generates examples — `library/socket/` 231, `core/kernel/` 58 — where
`SocketSpecs.each_ip_protocol do |family, ip_address|` binds names the harness
cannot produce without running the loop. Refusing them is the correct answer and
the one this slice deliberately chose over inventing `nil`.

The `language/` blocker it replaced is `NameError: uninitialized constant
LangSendSpecs` (71), which is honest and actionable: `language/fixtures/send.rb`
needs attribute assignment to compile.

### A calling-convention bug, surfaced and fixed

Unblocking `def_spec.rb`'s `describe` body revealed a real disagreement in a
subsystem this slice never set out to touch — the outcome
`docs/prd/0020-spec-tags.md` says to expect. `spinel run` on a plain file:

```ruby
def bar(a = b = c = 1, d = 2); [a, b, c, d]; end
bar        # ruby [1, 1, 1, 2]   spinel [1, 1, 1, nil]
bar(3, 4)  # ruby [3, nil, nil, 4]   spinel [3, nil, nil, nil]
```

`d` was wrong even when passed explicitly, so it was the binder rather than the
defaults. A parameter's default may declare a local, and Prism lists that local
where it is *written* — before the parameters that follow it — so `d` sat at
index 2 while the binder addressed index 1. `param_slot` bound it there under a
fresh name the body never read.

It now moves the parameter into binder position instead. The existing shadow
path stays for the case it was written for, a repeated `_`, where the name is
*earlier* than its position and `def m(_, _)` must still read the first. Nine
rows in `tests/eval.txt` cover both, measured against ruby 4.0.6 by
`scripts/eval-oracle.rb`.

### The definition of done

- [x] A local declared at a spec file's top level reaches the examples below it
- [x] A local declared in a `describe` and assigned in a `before` reaches them
- [x] `language/` no longer attributes any example to an enclosing local
- [x] No example that passed before stopped passing
- [x] `verify-passes.rb` replays what ran, and all 900 agree
- [x] Unit tests for both shapes and for the DSL exclusion

### Left for later

- **The loop-parameter shape**, 310 examples. `each do |family, ip_address|`
  around a `describe` needs the loop run, not another scope collected. Filed
  separately rather than folded in here, because refusing it is currently
  correct.
- **`language/fixtures/send.rb`**, 71 examples behind an attribute assignment
  the compiler does not have yet.

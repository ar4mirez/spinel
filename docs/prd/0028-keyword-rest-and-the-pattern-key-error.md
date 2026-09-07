# PRD 0028 — keyword rest, and the key a hash pattern could not find

Issues: [#193](https://github.com/ar4mirez/spinel/issues/193) · closes the two
items [PRD 0027](0027-super-globals-source-keywords-patterns.md) left open ·
Phase 1 · `area:engine`

## Objective

PRD 0027 landed four slices and said plainly that two things it had promised
were not delivered. This is those two.

1. **`**kw`, `m(**h)`, and `m("a" => 1)`** — #193, and the reason
   `language/fixtures/super.rb` would not compile, which #187's definition of
   done had named. 101 examples on its own reason plus 52 behind `SuperSpecs`.
2. **`NoMatchingPatternKeyError`** — the one example PRD 0027 tagged.

## Baseline

Measured on `main` at 443722e, which is PRD 0027 merged.

| | |
|---|---|
| Rust tests | 267 passing · 0 failing |
| ruby/spec corpus | 25,624 examples · **1,798 passed** · 0 failed |

## Decisions

### The keyword group becomes one `Hash`, or it stays a symbol list

`CallSite::keywords` is a list of symbol indices and the values ride the stack
in that order. A `**splat` and a non-symbol key are exactly the two shapes that
cannot go in it — which is why they were three refusals for one construct.

Rather than replace the list, a call whose keyword group contains either is
lowered *whole* to a single hash literal, and the site sets `kwsplat` instead.
The two are never both present, so nothing has to merge them — and merging is
where the subtlety is, because the order depends on where the splat was
written:

```ruby
def m(**kw) = kw.keys
m(k: 1, **{b: 2})  #=> [:k, :b]
m(**{b: 2}, k: 1)  #=> [:b, :k]
m(**{a: 1}, **{a: 2})  # {a: 2} — the last key written wins
```

`Hash` already decides all of that. An ordinary `m(k: 1)` keeps the
allocation-free path and is byte-for-byte the same bytecode as before.

### The binder hands `**kw` an `Array`, and the prologue makes it a `Hash`

Only the binder can see which keywords no named parameter claimed. But a `Hash`
is `core/hash.rb`, and the binder is Rust running inside argument assembly,
where re-entering the interpreter is the thing `expand_splats` already refuses
to do.

So the binder writes an `Array` of `[key, value]` pairs into the `**kw` slot,
and the method's prologue — where sending is already what the frame does —
calls `Hash.__from_pairs__` on it. One send, at a point that has one anyway.

### `Pending::keywords` is keyed by `Value`, not `SymbolId`

`m("a" => 1)` has to reach a `**kw` intact. A symbol key is `Value::symbol`, an
immediate, so matching a declared keyword is still one integer compare.

### The key error is a *form* rule, not a pattern rule

Measured on ruby 4.0.6, and not what reading the pattern would suggest:

```ruby
case {a: 1}; in {b: 2}; end              # NoMatchingPatternKeyError
case {a: 1}; in {b: 2}; in {c: 3}; end   # NoMatchingPatternError
{a: 1} => {b: 2}                         # NoMatchingPatternKeyError
{a: 1} in {b: 2}                         # false, and no error at all
```

So it is not "a hash pattern that misses a key raises the key error" — it is
"a form with *one* pattern reports which key was missing". The compiler opens a
hidden slot only for those forms; a hash pattern writes the key it did not find
into it, and the no-match path branches on whether anything did.

Writing at *every* miss gives "the last one wins" for free, which is what
`in {b: 2} | {c: 3}` naming `:c` means. Nesting needs nothing extra: the
message names the outer subject and the inner key, and the outer subject is the
one the general error already uses.

## Plan

1. `ParamSpec::kwrest`/`no_keywords`, `CallSite::kwsplat`. ✅
2. `Hash.__from_pairs__`, and the prologue that calls it. ✅
3. `Pending::keywords` keyed by `Value`; `pop_call` expands a `**` Hash. ✅
4. `bind_keywords`: collect the leftovers, or raise the way Ruby does. ✅
5. The key-error slot, opened only for a one-pattern form. ✅
6. Fifteen oracle rows for the Ruby 3 separation cases; Rust tests for the raises. ✅

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| corpus passed | 1,798 | **1,823** |
| corpus failed | 0 | **0** |
| `language/` passed | 1,083 | **1,097** |
| `verify-passes.rb` on the whole corpus | 1,798 agree | **1,823 agree** |
| Rust tests | 267 | **267** |

| reason | before | after |
|---|---|---|
| `a keyword rest parameter` | 101 | **0** |
| `a non-symbol keyword argument` | 45 | **0** |
| `a double-splat argument` | 10 | **0** |

Per file, measured on both sides:

| file | before | after |
|---|---|---|
| `language/super_spec.rb` | 5 | **11** |
| `language/method_spec.rb` | 12 | **16** |
| `language/proc_spec.rb` | 28 | **29** |
| `language/keyword_arguments_spec.rb` | 2 | **4** |
| `language/pattern_matching_spec.rb` | 85 | **86**, and its tag deleted |
| `language/block_spec.rb` | 91 | 91 |

### Definition of done

- [x] `**kw` binds the keywords no named parameter claimed
- [x] `m(**h)`, including mixed with named keywords and a positional splat
- [x] A non-symbol key reaches the callee
- [x] `**nil` refuses keywords — "no keywords accepted", measured
- [x] Rows in `tests/eval.txt` for the Ruby 3 separation cases
- [x] None of the three reasons appears in `scripts/spec.sh --blocked=0`
- [x] `language/{keyword_arguments,proc,block,method}_spec.rb` do not regress
- [x] `NoMatchingPatternKeyError` where Ruby raises it, and only there; the tag
      in `spec/tags/language/pattern_matching_tags.txt` is deleted

### `super.rb` compiles; `SuperSpecs` is now blocked on something else

The fixture's `Iseq` builds — #187's requirement — and its constants start
being defined. It stops partway at `define_method`, which the VM does not have,
so the constants *after* that line are still missing: 45 examples across nine
`SuperSpecs::*` names, where there were 52 behind one name before.

That is progress reported honestly rather than a finished job: the refusal
#193 owned is gone, and what is left is `Module#define_method`, worth **60**
examples corpus-wide and belonging to Phase 2's reflection slice.

### One failure the unblocking revealed

`core/proc/fixtures/common.rb` compiles now, and with it an example asserting
that `ProcSubclass.new` without a block raises `ArgumentError`. It does not: a
subclass of `Proc` has no built-in class id, so `Class#allocate` gives it the
plain-object shape and `new` answers an object.

Routing such a class to its built-in ancestor's allocation arm was tried and
**measured at −45 examples** — a `Hash` subclass loses its own class, and its
`default` with it — so it was reverted. The representation a subclass of a
built-in gets is its own slice; the example is tagged with that measurement in
the reason.

### Left for later

- **`ruby2_keywords`**, 30 examples, which is the Ruby 2 → 3 bridge and needs a
  flag on the `Hash` a splat carries.
- **A `Hash`-valued row still cannot live in `eval.txt`**, so every keyword row
  answers `.keys`, `.size` or `[]` instead. `Hash#inspect` writing `{:a => 1}`
  where ruby 4.0 writes `{a: 1}` is the reason, and it is not this slice's.

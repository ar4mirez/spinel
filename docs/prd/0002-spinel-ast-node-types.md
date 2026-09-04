# PRD 0002 — `spinel-ast`: node types covering Prism's full node set

Tracks [#2](https://github.com/ar4mirez/spinel/issues/2). Milestone: Phase 0: skeleton. `P0`, `size:L`, `area:parser`.

## Objective

Define the tree every crate above the parser reads. This is the boundary that keeps
Prism out of the rest of the repo: once `spinel_ast` exists, the lowering in
[#3](https://github.com/ar4mirez/spinel/issues/3), the bytecode compiler in phase 1,
and `spinel fmt` in phase 7 all consume the same types, and swapping Prism for a
hand-written parser stays a one-crate change.

The slice ships **types and no behaviour**. Nothing parses, lowers, or walks yet.
The value is that the shape is decided once, with Prism's full node set in front of
us, rather than discovered one missing variant at a time during the lowering.

## Non-goals

- The lowering itself, and `spinel parse file.rb`. That is #3, which this unblocks.
- A `prism` dependency anywhere. #3 adds it, to `spinel-parse` only.
- A visitor. Nothing walks the tree yet: #3 *builds* it, and the first real walker is
  the bytecode compiler. A visitor written now would be a shape guessed without a
  consumer. `docs/architecture.md` promised one and has been corrected.
- Byte-exact formatting fidelity. See R3.
- Serialization. The bytecode cache (phase 3) caches bytecode, not trees.

## Users

| User | Needs from this slice |
|---|---|
| #3, the lowering | A destination for all 151 Prism nodes, and a way to prove none was missed |
| The bytecode compiler (phase 1) | One assignment path, not thirty-one; spans for every construct it can reject |
| `spinel lint` (phase 7) | Somewhere to hang CRuby's own warnings: unused variable, duplicated argument, duplicated key |
| A future hand-written parser | A target that owes Prism nothing structurally |

## Requirements

### R1 — Every Prism node kind has a home, provably

Prism 1.9.0 defines 151 node kinds. `spinel_ast::prism_map::PRISM_NODES` has one row
per kind, naming the type it becomes and, when that type also serves other kinds, why.

The table is data rather than prose because prose goes stale silently. Four tests fail
if a row is missing, duplicated, unsorted, or blank, and `PRISM_NODE_COUNT` fails the
build on a Prism upgrade that adds a node.

It is a table of strings, not a `match` on Prism types, because `spinel-ast` may not
depend on `prism`. #3 reads the same list to prove its lowering is total.

### R2 — Fold where Ruby does not care, keep where it does

61 `ExprKind` variants cover 151 Prism kinds: 85 land one-to-one, 66 fold.

The fold is concentrated in one place. `ExprKind::Assign` absorbs **31** Prism nodes —
the `{Class,Constant,Global,Instance,Local}Variable × {Write,AndWrite,OrWrite,
OperatorWrite}` grid, the `ConstantPath` and `Call` and `Index` variants of the same
grid, and `MultiWriteNode` — because `@a ||= 1` and `A ||= 1` differ only in their
target, and the compiler wants one path through compound assignment, not thirty-one.
The variable kind is not lost; it moves into `Target`.

A fold must not cost information. `until` keeps a flag rather than becoming `while !`,
an integer keeps its base, and `unless` keeps its keyword, because those are choices a
reader made. Where Prism caches a predicate its own C consumers want —
`ArrayNodeFlags::CONTAINS_SPLAT`, `ParameterFlags::REPEATED_PARAMETER` — the flag is
dropped, because a walk of the children answers it and a second copy can disagree with
the tree. `prism_map`'s module docs list every such case.

### R3 — Spans on everything a diagnostic points at

Uniformly: a `span` beside a `kind`. `Expr`, `Target`, `HashEntry`, and each parameter
follow that rule, so a consumer learns `.span` once.

The set is not arbitrary. Ruby aims a warning at each of them, and Spinel is committed
to full compatibility:

| CRuby warning | Needs a span on |
|---|---|
| `assigned but unused variable - a` | `Target` |
| `key :a is duplicated and overwritten` | `HashEntry` |
| `duplicated argument name` | each parameter |

This is a semantic tree, not a lossless syntax tree. `then`, `do` versus `{}`, the
parens in `def foo()`, and comments are not nodes; only the span survives. That is
enough for the compiler, for lint, and for diagnostics. `spinel fmt` needs byte-exact
trivia and will want a token layer beside this tree rather than more fields on it —
recorded as a follow-up rather than guessed at now.

### R4 — Prism stays inside `spinel-parse`, enforced

`spinel-ast` has no dependencies at all, and `#![forbid(unsafe_code)]`. The CI
`layering` job added in #1 already fails any crate other than `spinel-parse` taking a
direct `ruby-prism` dependency; this slice keeps that job passing and relies on it
rather than adding a second check.

## Definition of done

The issue's three boxes, plus what the repo's own rules add:

- [x] Every Prism node kind has a counterpart or a documented reason it is folded —
      151 rows, checked by `node_count_matches_prism` and `every_node_has_a_home`
- [x] Types are `pub` from `spinel-ast` and carry source spans — 77 public types
- [x] No `prism` dependency outside `spinel-parse` — CI `layering` job green
- [x] `cargo test --workspace` passes: 16 tests, 8 of them new
- [x] `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings` clean
- [x] `docs/architecture.md` reconciled with what shipped

No ruby/spec delta: this slice ships no semantics, so no spec can newly pass. The
check that stands in for it is R1's coverage table, which is what the roadmap bullet
for this slice actually names.

## Open decisions for the owner

1. **The 31-node assignment fold.** The largest judgement call here. It makes the
   compiler simpler and `a.b ||= 1` uniform with `@a ||= 1`, at the cost that a reader
   of `ExprKind` no longer sees Ruby's assignment forms enumerated. Reversible while
   the lowering is unwritten; expensive after phase 1.
2. **`Name = Box<str>`.** An interned symbol id would be faster and is what
   `spinel-vm` will want. Deferred because the symbol table is #6 and the alias keeps
   the swap to one line. Marked `ponytail:`.
3. **Prism 1.9.0 as the pinned node set.** Latest release. If Ruby 4.0 ships against a
   different Prism, `PRISM_NODE_COUNT` is where that shows up.
4. **No visitor.** See non-goals. Say so if you would rather have one before #3.

## Tasks

| # | Task | Proves |
|---|---|---|
| T1 | Enumerate Prism 1.9.0's node set and per-node semantic fields from upstream `config.yml` | 151 nodes, 15 flag sets |
| T2 | Design the fold: which kinds collapse, and what carries the distinction | R2 |
| T3 | Write `lib.rs`: `Span`, `Expr`/`ExprKind`, and the supporting types | `cargo build` |
| T4 | Write `prism_map.rs`: the coverage table and its four tests | R1 |
| T5 | Audit the design field-by-field against Prism for dropped semantics | audit table below |
| T6 | Construction test building a real method by hand | `a_real_method_is_expressible` |
| T7 | Reconcile `docs/architecture.md` | doc diff |

## What the audit caught

Found by diffing every Prism node's non-location fields against the design, and by
asking what a diagnostic would have to point at. Each was fixed in this PR.

### Correctness

**A1 — Three CRuby warnings had nothing to point at.** *(the real find)*
The first draft spanned `Expr` and nothing else, on the reasoning that every construct
sits inside some expression. That holds for the compiler and fails for diagnostics:
`duplicated argument name` names one parameter out of a list, `key :a is duplicated and
overwritten` names one entry out of a hash, and `assigned but unused variable` names
the target rather than the assignment. Each would have underlined the enclosing `def`,
`{`, or `=`.

Full Ruby compatibility includes the warnings, so this would have surfaced in phase 1
as a spec failure with no cheap fix: adding spans to parameters after the lowering
exists means touching every construction site. `Target`, `HashEntry`, and the five
parameter types now carry spans, and the `{ span, kind }` pairing is uniform.

**A2 — String content cannot be `String`.** `"\xFF"` is a valid one-byte Ruby String
and `:"\xFF"` a valid Symbol, so literal content is `Box<[u8]>`. A `String` field would
have compiled, passed a smoke test, and made every binary-encoded literal in
`core/string/` unrepresentable. Locked in by `string_content_is_bytes_not_utf8`.

**A3 — `Rational` dropped its base.** Prism carries `IntegerBaseFlags` on
`RationalNode`; the draft kept the base on integers and forgot it on rationals. Added,
for the same reason integers have it.

### Clarity

| | Found | Fixed to |
|---|---|---|
| C1 | Prism encodes a pattern guard by wrapping the pattern in an `IfNode`, so `in a if b` puts an `if` where the consumer expects a pattern. Passing that through would make every pattern matcher unwrap a conditional first. | `InClause { pattern, guard: Option<Guard> }`. The lowering lifts it back out. |
| C2 | Spanned types disagreed on shape: some would have had `span` first, some a wrapper. | One rule — `span` beside `kind` — stated in the crate docs and followed by all four. |
| C3 | Parameters as `Option<ParamList>` plus `it`/numbered booleans admits states Ruby cannot have, such as numbered *and* explicit. | `Params` is an enum: `None`, `Explicit`, `Numbered(u8)`, `It`. |
| C4 | `frozen` and `mutable` as two booleans, per Prism's flags, admits both set at once. | `frozen: Option<bool>`; `None` is "the file said nothing". |
| C5 | "Every node is covered" as a prose claim in a doc comment goes stale on the first Prism upgrade and nobody notices. | A table plus four tests, and a `PRISM_NODE_COUNT` that fails the build. |

### Verified, no change needed

- The `layering` CI job from #1 catches Prism escaping `spinel-parse`; ran locally
  against the current tree and it passes. No second check added.
- Folds are the minority: 85 of 151 kinds are one-to-one. `folds_are_the_minority` is
  the tripwire for a future slice folding until the tree stops resembling Ruby.
- The recursive types are actually constructible. `a_real_method_is_expressible`
  builds a `def` with required, rest, keyword, and block parameters, an `||=` to an
  ivar, and an interpolated string written through an index target. A tree of types
  nobody has instantiated can be missing a `Box` and nobody finds out until the
  lowering is half written.
- `Debug` is derived throughout, which is what `spinel parse` prints in #3.

### Performance

The AST is allocated once per file and walked repeatedly, so node size is the number
that matters, and `spinel-ast` is a dependency of every other crate, so its compile
time is on every incremental build.

| | |
|---|---|
| `Expr` | **40 bytes** (`ExprKind` 32 + `Span` 8) |
| `Target` | 32 bytes |
| incremental rebuild of `spinel-ast` | 0.10 s, median of 3 |

Every payload past two words is boxed, and `expr_stays_small` fails if that slips.
`ExprKind::Var` and `ExprKind::Int` are deliberately **not** boxed even though they set
the 32-byte width: variable reads are the most common node in any Ruby program, and
boxing them would trade 8 bytes of enum width for an allocation each.

## Follow-ups filed

- A token or trivia layer for `spinel fmt`, so R3's dropped punctuation has a home
  before #136 starts. Noted on that issue rather than filed separately.
- A visitor, when the bytecode compiler gives it a consumer.

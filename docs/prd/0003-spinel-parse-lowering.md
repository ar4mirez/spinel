# PRD 0003 — `spinel-parse`: lower Prism to `spinel_ast`, plus `spinel parse`

Tracks [#3](https://github.com/ar4mirez/spinel/issues/3). Milestone: Phase 0: skeleton. `P0`, `size:M`, `area:parser`.

## Objective

Turn Ruby source into a `spinel_ast::Program`, and give the repo a way to look at the
result. [#2](https://github.com/ar4mirez/spinel/issues/2) defined the destination
types; this slice fills them, and adds `spinel parse` so every later slice —
the bytecode compiler in phase 1, `spinel lint` in phase 7 — has a window onto
the tree it consumes rather than a `Debug` dump it has to decode.

It is also the first slice that touches real Ruby at scale. The check is not a
unit test: it is 5,026 files of ruby/spec and the pure-Ruby stdlib reaching
`spinel_ast` without a single node the lowering does not handle.

## Non-goals

- **Semantics.** Nothing runs. A `Program` is a shape, not a program.
- **A visitor.** Still deferred, for the reason [PRD 0002](0002-spinel-ast-node-types.md)
  gave: the bytecode compiler is the first real walker, and it should shape the
  visitor. This slice walks Prism, not `spinel_ast`.
- **Replacing Prism.** The hand-written parser stays a later option. What this
  slice buys is that the option stays a one-crate change.
- **Byte-exact trivia.** Comments, `then`, `do` versus `{}` — still dropped, still
  a token layer's job when `spinel fmt` needs one.
- **Formatting or linting output.** `spinel parse` prints a tree. It does not
  judge the code.

## Users

| User | Needs from this slice |
|---|---|
| The bytecode compiler (phase 1) | A tree for every construct in `language/`, with spans that point where a diagnostic would |
| Whoever debugs that compiler | To read a tree in one screen, not forty lines per statement |
| CI | One command that fails when a Ruby file stops lowering |
| `spinel run` (phase 3) | Syntax errors it can print to a user, told apart from Spinel's own bugs |

## Requirements

### R1 — Every Prism node lowers, proven by the compiler

`Lower::kind` matches `ruby_prism::Node` exhaustively. There is no `_ =>` arm.
A Prism upgrade that adds a node kind is a build failure in this file, which is
the strongest form the guarantee can take: it cannot be forgotten, and it cannot
go stale the way a comment does.

Thirty-one Prism nodes never appear in expression position: twenty-seven that a
parent owns — an `ArgumentsNode` inside a call, a `WhenNode` inside a `case` —
three target nodes with no reading as an expression (`CallTargetNode`,
`IndexTargetNode`, `MultiTargetNode`), and `ProgramNode`, which is the root.
Reaching one there means this crate has a bug, so it produces a `Diagnostic`
with `Origin::Lowering` and an `ExprKind::Missing` hole rather than a panic. That is
the "unhandled node" the corpus sweep looks for, and the reason a sweep over a
5,000-file corpus reports every bad file instead of aborting at the first.

### R2 — Folds keep meaning

`spinel_ast::prism_map` is the ledger of what folds into what; this slice is
where the ledger becomes code. The tests state the cases where a fold could
quietly lose something:

| Fold | What must survive | Test |
|---|---|---|
| 31 assignment nodes → `Assign` | the operator and the variable kind | `every_assignment_form_folds_into_one_shape`, `assignment_targets_keep_the_variable_kind` |
| `UntilNode` → `While` | `until` and `begin ... end while` | `until_keeps_its_keyword_rather_than_becoming_not_while` |
| `UnlessNode` → `If` | the keyword | `unless_keeps_its_keyword` |
| `ForwardingSuperNode` → `Super` | bare `super` forwards, `super()` does not | `bare_super_is_not_super_with_no_arguments` |
| `KeywordHashNode` → `Hash` | `braces: false` | `hash_entries_carry_spans_for_the_duplicate_key_warning` |
| interpolated ↔ plain literals | one flat list of runs and holes | `interpolation_is_a_flat_list_of_runs_and_holes` |

### R3 — Errors are attributed

`Parsed` carries a tree *and* diagnostics, always, because Prism recovers from
syntax errors and both halves have a reader. Each `Diagnostic` names an
`Origin`:

- `Syntax` — the source is not valid Ruby. `spinel run` shows this to a user.
- `Lowering` — the source is fine and this crate could not lower it. Nobody
  should ever see one.

The distinction is load-bearing rather than decorative: ruby/spec ships files
that are *deliberately* invalid Ruby (`command_line/fixtures/bad_syntax.rb`,
`core/exception/fixtures/syntax_error.rb`). Without it, a corpus sweep either
fails forever on those two files or cannot fail at all.

### R4 — `spinel parse` is readable enough to debug with

The derived `Debug` prints `1 + 2` as forty lines, because a Ruby tree is mostly
wrappers and `{:#?}` shows every one. The default output is one line per node:

```text
program                              30..194
└─ class                             30..194
   ├─ name: const Greeter            36..43
   ├─ superclass: const Base         46..50
   └─ body: def greet                53..190
      ├─ params:
      │  ├─ req name                 63..67
      │  └─ key greeting:            76..90
      │     └─ str "hi" frozen       86..90
      └─ body: assign ||=            102..114
         ├─ target: ivar @seen       102..107
         └─ value: hash              112..114
```

Rules that fell out of using it:

- One line per node, spans right-aligned in their own column, so the shape reads
  as a shape and the offsets can be skipped.
- Slots are named — `recv:`, `arg:`, `then:`, `else:` — because position alone
  does not say which is which.
- A slot holding one statement inlines into the parent's line; several get a
  heading. `else: call raise` beats an `else:` line with one child under it.
- Literals print as they were written. `0xff_ff` reads back as `0xffff`, not as
  `0x65535`.
- `--format debug` keeps `{:#?}` for the times a field is missing rather than
  merely unprinted.
- Colour only when stdout is a terminal, and never when `NO_COLOR` is set.

### R5 — The sweep is the definition of done

`spinel parse <dir>` walks every `.rb` file under a directory, reports only what
failed, and exits non-zero on unhandled nodes and never on syntax errors. CI
runs it against ruby/spec and ruby/ruby's `lib/`, both pinned, both cloned
rather than vendored — vendoring each is its own roadmap slice. The job already
sweeps `spec/ruby/` and `stdlib/` when they exist, so those slices do not have
to edit it.

## Definition of done

The issue's three boxes, plus the repo's own rules:

- [x] `spinel parse file.rb` prints the tree — and `--format debug` prints the other one
- [x] Every file under a ruby/spec and stdlib corpus lowers without an "unhandled node" error — 5,026 files, 0 unhandled
- [x] A CI job runs that sweep — `sweep` in `.github/workflows/ci.yml`
- [x] No `prism` dependency outside `spinel-parse` — the `layering` job, green
- [x] `cargo test --workspace` passes: 57 tests, 41 of them new
- [x] `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings` clean
- [x] `docs/cli.md` and `docs/architecture.md` reconciled with what shipped

No ruby/spec delta: this slice still ships no semantics, so no spec can newly
pass. The sweep is what stands in for it, and it is the check the roadmap bullet
for this slice actually names.

## Corpus result

| Corpus | Files | Unhandled nodes | Syntax errors |
|---|---|---|---|
| ruby/spec @ `620a912` | 4,437 | 0 | 2 (both deliberate fixtures) |
| ruby/ruby `lib/` @ `5efd4ad` | 589 | 0 | 0 |

## Performance

7.7 MB of Ruby across 4,437 files parses and lowers in **0.26 s** wall, about
25 MB/s and 17,000 files/second, single-threaded. The release binary is 1.5 MB.

No parallelism. `rayon` would divide a number nobody is waiting on, and the
sweep is the only thing that ever sees more than one file. Worth revisiting when
`require` (phase 3) parses a whole dependency tree at boot, not before.

`ruby-prism` builds Prism's C and runs `bindgen`, which adds about 12 s to a cold
build of the workspace and nothing to a warm one. Accepted: it is the cost of not
writing a Ruby parser this year, and it is contained in one crate.

## Open decisions for the owner

1. **Diagnostics live here rather than in a `spinel-diagnostics` crate.** `Diagnostic`
   is a span and a string. `spinel run` and `spinel lint` will both want a richer
   one — notes, suggestions, related spans. Splitting later is cheap while there
   is one producer; it is not once there are four.
2. **`Origin::Lowering` diagnostics are returned, not panicked on.** The tradeoff
   is that a bug in this crate is silent unless someone looks at `errors`. The
   sweep is what looks. The alternative — `unreachable!()` — makes `spinel parse`
   over a corpus stop at the first bad file, which is the wrong shape for the
   check this slice exists to provide.
3. **`IntValue::Big` carries a leading `-` for negative bignums.** PRD 0002 says
   "digits, without separators or prefix", and a sign is neither; dropping it
   would lose the value, since `Big` has no other place to put it. Hex digits are
   lowercase, which is a canonical form rather than what the file said.
4. **The tree printer lives in `spinel-cli`, not `spinel-ast`.** PRD 0002 kept
   `spinel-ast` to types with no behaviour. `spinel parse` is its only consumer;
   move it down if a second appears.
5. **CI clones two corpora at pinned SHAs.** Pinned so an upstream commit cannot
   turn a green PR red. The cost is that the pins need bumping deliberately, and
   that new Ruby syntax landing upstream will not be noticed until they are.

## Tasks

| # | Task | Proves |
|---|---|---|
| T1 | `ruby-prism` in `spinel-parse` only, exhaustive `match` over `Node` | R1, `layering` job |
| T2 | The 31-node assignment fold, via four macros rather than 31 blocks | R2 |
| T3 | `Parsed` with `Origin`-tagged diagnostics | R3 |
| T4 | Bignum digits from Prism's base-2^32 limbs | `bignums_survive_as_digits_in_their_own_base` |
| T5 | The tree printer and `--format debug` | R4 |
| T6 | `spinel parse <dir>` sweep, and the CI job that runs it | R5 |
| T7 | Sweep ruby/spec and the stdlib; fix what it finds | the corpus table above |
| T8 | Reconcile `docs/cli.md` and `docs/architecture.md` | doc diff |

## What the audit caught

Three of these came from running the sweep, which is the argument for making the
corpus the check rather than a unit-test suite: none would have been guessed.

### Correctness

**A1 — Pattern bindings arrived as target nodes.** *(11 files)*
`in [x, *rest]` hands over a `LocalVariableTargetNode` where `ArrayPattern`
wants an `Expr`, because in a pattern a name is a binding, not a read. The first
draft reported all eleven as unhandled nodes. A variable target in expression
position now lowers to the variable it names, which is what the binding means.

**A2 — `{ |(a, b)| }` nests parameters inside a multi-target.** *(6 files)*
Every other multi-target holds `*TargetNode`s; a destructuring block parameter
holds `RequiredParameterNode`s. `Lower::target` now reads both.

**A3 — `in [0, 1, ]` has an implicit rest.** *(1 file)*
The trailing comma means "and anything else". It lowers to `Splat(None)` — a
splat that binds nothing — rather than an unhandled `ImplicitRestNode`.

**A4 — `0xff_ff` printed as `0x65535`.** Found by reading the output rather than
by the sweep. The printer prefixed `0x` onto a decimal string. A literal now
prints in the base it was written in.

**A5 — A mistyped subcommand answered as a Ruby file.** `spinel pasre` replied
"cannot run `pasre` — this build has no VM yet", sending the reader after a file
that was never the point. A bare argument is now read as a file only if it is
named `.rb` or exists on disk; anything else is an unknown subcommand.

### Clarity

| | Found | Fixed to |
|---|---|---|
| C1 | The tree printed `name: target: const Greeter` — `target()` named its own slot and the caller named it again. | `target()` returns unnamed; every caller says which slot it fills. |
| C2 | "unhandled node: node in target position in expression position", from composing two half-messages. | Each site writes its whole message. |
| C3 | The sweep counted a deliberate bad-syntax fixture as a failure, so it could never be green on ruby/spec. | `Origin`, and two counts in the summary. |
| C4 | A sweep over a broken corpus would print thousands of lines. | Twenty per category, then `... and N more`. |

### Verified, no change needed

- **Spans are real everywhere.** `parameters_carry_their_own_spans` and
  `spans_point_at_the_thing_a_diagnostic_would_underline` check the three CRuby
  warnings PRD 0002 added spans for. Prism's own warnings already come out
  pointing at the right token — `assigned but unused variable - rest` underlines
  `rest`, not the pattern.
- **`Expr` is still 40 bytes.** `expr_stays_small` in `spinel-ast` passes
  unchanged; nothing here grew a payload.
- **Binary-encoded literals survive.** `string_content_is_bytes_not_utf8`, and a
  whole-file check that a non-UTF-8 source does not panic.
- **No process-global state.** `Lower` is a struct passed by `&mut`; the
  `layering` job's grep is clean.

## Follow-ups

- A `spinel-diagnostics` crate, when `spinel run` and `spinel lint` both want
  notes and suggestions on a diagnostic. See open decision 1.
- Parallelism in the sweep, if a later phase makes 0.26 s matter.
- Replace the two CI clones with `spec/ruby/` and `stdlib/` once
  [#4](https://github.com/ar4mirez/spinel/issues/4) and
  [#5](https://github.com/ar4mirez/spinel/issues/5) vendor them. The job already
  sweeps both when present.

# PRD 0033 — the largest "compiler gap" is the harness, and the ranking that hid it

Issue: [#220](https://github.com/ar4mirez/spinel/issues/220) · Phase 1 · `area:infra`

## Objective

#220 is a triage issue with three boxes: decide whether the Phase 1 milestone
target moves, file the untracked compiler refusals as slices largest first, and
close once the scope is settled.

Its ranking table is headed by **310 examples — "a local variable from an
enclosing scope"** — and states:

> the first is worth a second look on its own: at 310 examples it is the single
> largest compiler gap in the corpus, and it is *not* #164 — that one was the
> harness failing to see an enclosing local and is closed. This is the compiler.

That claim is wrong, and filing a `size:L` compiler slice against it would have
been the third time this reason misdirected a slice. This PRD checks the ranking
before filing from it, corrects the row, and removes the ambiguity that produced
it.

## Baseline

Measured on this branch at 2256272, before any change. Corpus counts are
`--platform=linux`, which is what `scripts/spec-status.sh` pins.

| | |
|---|---|
| ruby/spec | 3835 files · 25624 examples · 2158 passed · 0 failed · 21547 blocked · 1919 skipped |
| `language/` | 2735 examples · 1312 passed · 0 failed · 1356 blocked · 67 skipped |
| Rust tests | 10 passing in `tests/bytecode.rs` |
| top blocked-by refusal | 310 · "a local variable from an enclosing scope" |
| `bench/spec-status.md` | current, regenerates clean |

`language/` at 1312 / 2735 = 48% reproduces #220's figure exactly, so the
milestone half of the issue was measured on code that has not moved.

## The premise was checked before it was built on

`spec/tags/README.md`, this project's memory, and PRD 0024's own first plan step
all say the same thing: a blocked reason can name the wrong subsystem, so run
the shape as a plain `.rb` file first. Ten seconds:

```ruby
x = 1
[1].each { puts x }                                  # 1
[[1,2]].each { |a, b| [1].each { puts a + b } }      # 3
def m; y = 5; [1].each { puts y }; end; m            # 5
outer = 10
[1].each { [2].each { [3].each { puts outer } } }    # 10
```

Every one is correct. The compiler reads enclosing locals fine.

### Where the 310 actually come from

`outer_slot` refused on four paths that all reported the same string. Giving
each a temporary distinct marker and re-running the corpus split them:

| examples | path | reachable from |
|---:|---|---|
| 282 | flattened, resolved-never-created | `flattened_expression` only |
| 28 | name missing from the outer scope's list | `flattened_expression` only, in practice |
| 0 | `for`-body depth cannot be translated | either |
| 0 | depth points past the outer chain | either |

`compile::flattened_expression` has exactly one caller in the repository —
`spec/harness/src/run.rs:454`. The `spinel` binary never takes that path. So
every one of the 310 is the harness.

The 28 land in `core/kernel/Float_spec.rb` (13), `core/kernel/Integer_spec.rb`
(11) and `library/socket/tcpsocket/gethostbyname_spec.rb` (4). Their shape:

```ruby
%w(x X).each do |x|                                     # Integer_spec.rb:305
  it "parses the value as a hex number ... 0#{x}" do
    Integer("0#{x}1").should == 0x1
  end
end
```

`x` is bound by a loop the harness does not run. Compiled as a plain program the
same nesting prints `0x1`, `0X1`.

### PRD 0024 had already answered this

The shape, the count and the decision are all in
`docs/prd/0024-harness-enclosing-scope.md`, under **Left for later**:

> **The loop-parameter shape**, 310 examples. `each do |family, ip_address|`
> around a `describe` needs the loop run, not another scope collected. Filed
> separately rather than folded in here, because refusing it is currently
> correct.

and, in its results, the reason refusing beats resolving: inventing a slot binds
`nil`, which turned `core/symbol/inspect_spec.rb` from blocked into a *failure*
against two nils the harness had made up — with nothing guaranteeing the failing
direction rather than a false pass.

So the row is not a compiler slice, is not unowned, and is not new. It belongs to
[#145](https://github.com/ar4mirez/spinel/issues/145) — mspec running on Spinel,
Phase 2 — which runs the generator loops instead of collecting scopes.

## Decisions

### The two meanings get two strings

One reason string covered a compiler bug and a harness limitation. It read as
the former twice: #164 was filed at 133 examples, re-triaged at 338, and #220
re-filed the remainder at 310 as "the single largest compiler gap". Both times
the plain `.rb` shape ran fine.

`outer_slot` now picks its reason from `self.flattened`, which is set only by
`flattened_expression` and therefore only by the harness. The harness's miss
reports **"a local variable from a block the harness did not run"**; a genuine
one keeps the old string, and the `for`-body path is untouched because it is not
about flattening.

This is diagnostics, not semantics: the same examples block, and the corpus
totals are unchanged. What changes is that `scripts/spec.sh --blocked=0`, which
is how the next slice gets chosen, no longer offers one number that means two
things. The harness is deleted at the end of Phase 2, so the string is
temporary — but the ranking it feeds is being read for slice choice now.

### The `language/` milestone maths is untouched

`language/` reports **zero** examples under either string, so correcting the row
does not move #220's bucket table. Fixing all 438 engine-owned `language/`
blockers still lands at 1750 / 2735 = 64%, and 90% still needs #145 and #39.
The milestone is gated, exactly as #220 concluded.

### The corrected ranking, and what got filed

Every row below was re-run as a plain `.rb` file through `spinel` before being
filed, which changed two of them:

| examples | construct | filed | note |
|---:|---|---|---|
| ~~310~~ | ~~a local variable from an enclosing scope~~ | — | the harness; #145 |
| 55 | argument forwarding (`...`) | #221 | the real largest |
| 52 | a regexp encoding modifier | #222 | |
| 45 | a backtick command | #223 | |
| 15 | a safe-navigation call (`&.`) | #224 | |
| 13 | an integer wider than a fixnum | #225 | |
| 12 | a compound constant assignment | #226 | |
| 12 | a rational or complex literal | #227 | two constructs, one string |
| 12 | a splat in `when` | — | already #219 |
| 10 | an elided hash value (`{x:}`) | #228 | |
| 9 | a splat or keyword in an index target | #229 | op-assign only; plain assign works |
| 8 | a flip-flop | #230 | |
| 8 | symbol interpolation | #231 | |
| 6 | a regexp that writes its named captures to locals | #232 | |
| 5 | an anonymous block parameter | #233 | |
| 4 | `alias` on a global variable | #234 | |

Two rows were mis-stated by their own reason string, which is the same failure
as the 310 in miniature:

- **`h[*[:a]] = 2` compiles and runs.** Only the op-assign form,
  `h[*[:a]] += 1`, refuses. The keyword half of "splat or keyword" is a syntax
  error in CRuby too, so it is not a slice at all.
- **"a rational or complex literal"** is `3r` and `3i` — two literals behind one
  string, and a slice sized from the count would be sized for one.

`__ENCODING__` (3) is not filed: it waits on the Encoding class, Phase 2. The
four 1-example rows are left in the ranking rather than filed.

## Plan

1. Verify the top refusal as a plain `.rb` file. ✅
2. Split the four refusal paths with temporary markers; re-run the corpus. ✅
3. Confirm `flattened_expression`'s only caller is the harness. ✅
4. Give the harness path its own reason; regression-test both meanings. ✅
5. Re-run the corpus and `scripts/spec-status.sh`: no example moves. ✅
6. Verify each remaining row as a plain `.rb` file, then file it. ✅
7. Answer the milestone question on #220 and close it. ✅

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| corpus passed | 2158 | **2158** |
| corpus failed | 0 | **0** |
| corpus blocked | 21547 | **21547** |
| `bench/spec-status.md` | current | **regenerates byte-identical** |
| refusals naming the compiler that are the harness | 310 | **0** |
| Rust tests in `tests/bytecode.rs` | 10 | **11** |

No example changed state. The slice is a diagnostics correction, and its whole
delta is that the ranking now says which subsystem owns its largest row.

### The definition of done

- [x] The 310 row is attributed to the harness, with the caller graph to show it
- [x] The two meanings are distinguishable in `scripts/spec.sh --blocked=0`
- [x] A regression test asserts both strings against the shapes that produce them
- [x] No example that passed before stopped passing
- [x] Every filed row was reproduced as a plain `.rb` file first
- [x] The milestone question is answered on #220

### Left for later

- **The loop-parameter shape**, 310 examples, stays blocked and stays correct.
  It resolves when #145 runs mspec's generator loops.
- **`Range#select`** is missing, found while reaching for a flip-flop shape and
  not filed here; it is Phase 2 core-library work.

# PRD 0023 — capture-restore records: a loop iteration's captures survive backtracking

Issue: [#177](https://github.com/ar4mirez/spinel/issues/177) · Phase 1 · `area:engine`

## Objective

Make `spinel-regex` agree with Onigmo about which captures survive backtracking.

`spec/tags/language/regexp/empty_checks_tags.txt` skipped one example. Nineteen
of its twenty assertions already held; the twentieth did not:

```ruby
/^(()|a|())*?$/.match("aaa").to_a   # Ruby: ["aaa", "a", "", nil]
                                   # Spinel: ["aaa", "a", nil, nil]
```

## Baseline

Measured on `main` at 7e7c24a, in a worktree, before any change.

| | |
|---|---|
| Rust tests | 254 passing · 0 failing |
| ruby/spec | 25624 examples · 1291 passed · 0 failed · 1861 skipped |
| `language/regexp/` | 257 examples · 151 passed · 0 failed · 4 skipped |
| `empty_checks_spec.rb` | 4 examples · 3 passed · 1 skipped |
| `KNOWN_DIVERGENCES` | one entry, `^(()|a|())*?$` |

## Decisions

### The rule was measured across 47 patterns, not derived from Onigmo's source

The issue describes the rule — "a capture set by a loop iteration survives
backtracking out of that iteration, but a capture set outside a loop does not" —
and the temptation was to reconstruct `regexec.c`'s `STACK_POP` from memory.
`CLAUDE.md` answers behaviour questions from ruby/spec and then from what CRuby
*does*, so instead a corpus of 47 patterns was generated, each putting a capture
on a path that is later abandoned, in every structural position that could
plausibly matter: inside and outside loops, greedy and lazy, bounded and
unbounded, nested, behind lookaround and atomic groups, with and without
backreferences.

**41 of the 47 already agreed.** The six that did not are one shape: an anchored,
lazy repeat whose body's first alternative is an empty capture group.

That narrowed the question from "how does Onigmo restore captures" to "what
distinguishes these six", which is answerable.

### The commit is at the bottom of a completed iteration

Two measured cases fix the rule between them, and they pull in opposite
directions:

```ruby
/^(()|a)*?$/.match("aa").to_a  # ["aa", "a", ""]  — $2 survives
/((a)x|a)*/.match("aa").to_a   # ["aa", "a", nil] — $2 is rolled back
```

Both set a capture and then abandon the path that set it. The difference is
whether an **iteration completed in between**. In the first, iteration 1 ran
`()` to the bottom of the loop before the engine came back and took `a` instead.
In the second, `(a)x` failed part way through, so no iteration ever finished.

So the rule is not "loops never roll back" and not "backtracking always rolls
back". It is: *a completed iteration commits its captures.* `Inst::Progress`
already sits at the bottom of every iteration and already distinguishes the
completed case from the stalled one, so the commit went there — reaching exactly
the alternatives that iteration opened, which is everything above the depth its
`Mark` recorded.

Six lines, in the one place that already knew an iteration had finished.

### Snapshots stay; only their contents are updated

The issue notes that a restore-record scheme is likely cheaper than
snapshot-and-restore and that this is therefore not purely a correctness fix.
That rewrite was **not** done. Onigmo pushes a restore record only for the groups
that need one; this engine still clones the capture vector at each backtrack
point and now rewrites some of those clones on the way past.

The cheaper scheme is a real change to how the machine stores captures, and the
correctness question was answerable without it. Doing both at once would have
meant a performance rewrite validated by a rule that was still being pinned down.
The `ponytail` comment names the ceiling and the upgrade path, and the full spec
run is unchanged at 4.3–4.5s, so nothing is waiting on it.

## Plan

1. Generate the 47-pattern corpus; measure CRuby; find the real scope. — **done**
2. Commit a completed iteration's captures in `Inst::Progress`. — **done**
3. Delete `spec/tags/language/regexp/empty_checks_tags.txt`. — **done**
4. Empty `KNOWN_DIVERGENCES`; add the two cases as a named test; add all four
   patterns to the measured corpus in `oracle.txt`. — **done**

## Results

### ruby/spec delta

| | before | after |
|---|---|---|
| `empty_checks_spec.rb` | 3 passed · 1 skipped | **4 passed** · 0 skipped |
| `language/regexp/` | 151 passed · 0 failed · 4 skipped | **153 passed** · 0 failed · 2 skipped |
| whole corpus | 1291 passed · 0 failed | **1302 passed** · 0 failed |
| Rust tests | 254 passing | **257 passing** |
| `KNOWN_DIVERGENCES` | 1 entry | **empty** |
| capture corpus vs CRuby | 41 of 47 | **47 of 47** |

`language/regexp/` gains two rather than one because #178 is in the same branch.

### The definition of done

- [x] `language/regexp/empty_checks_spec.rb` passes in full, and the entry leaves
      `spec/tags/language/regexp/empty_checks_tags.txt` — the file is deleted
- [x] The oracle in `crates/spinel-regex/tests/oracle.rs` covers the two cases
      against CRuby, and the entry leaves `KNOWN_DIVERGENCES` — which is now
      empty. `capture_restore_records` names both directions, and all four
      patterns joined `oracle.txt`'s measured corpus so they are re-checked
      against a live CRuby in CI
- [x] `language/regexp/` does not regress — 151 → 153, 0 failed

### Verified by mutation, not just by green

Deleting the six-line commit fails both `capture_restore_records` and the corpus
replay `agrees_with_cruby_on_every_pattern_it_accepts` — the second is what
matters, because it means the fix is pinned by the measured table and not only by
the test written alongside it.

### Left for later

The restore-record rewrite itself, as a performance change: track only the groups
that live inside the loop, the way Onigmo does, instead of cloning the whole
capture vector per backtrack point. Nothing depends on it today.

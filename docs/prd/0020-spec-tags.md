# PRD 0020 — `spec/tags/`: skipped specs with a reason

Issue: [#146](https://github.com/ar4mirez/spinel/issues/146) · Phase 1 · `area:infra`

## Objective

Give the project's one sanctioned escape hatch a real file format, a reader that
enforces it, and a way to notice when it has gone stale.

`CLAUDE.md` says: *never mark a spec as "expected failure" to make a slice
green; skip it with a reason in `spec/tags/`*. `docs/engine.md` says the same.
[#158](https://github.com/ar4mirez/spinel/pull/158) built the smallest thing
that could honour that rule in the slice that first needed it —
`spec/tags/skip.txt`, one `description<TAB>reason` per line — and this issue was
left holding the parts that were deliberately guessed at or skipped:

- the layout is a single flat file, not mspec's per-spec-file one, so it does
  not survive [#145](https://github.com/ar4mirez/spinel/issues/145) replacing
  the harness with mspec;
- a reason is required by convention only; a line without one is silently
  dropped by `split_once('\t')`, which is the exact silent skip the rule exists
  to prevent;
- a tag naming an example that no longer exists is silently ignored, so the file
  rots the first time upstream rewords an `it`;
- there is no `README.md` saying a tag is a debt rather than a result.

## Baseline

Measured on `feat/146-spec-tags` at 41bcc9d, before any change.

| | |
|---|---|
| Rust tests | 237 passing · 0 failing |
| ruby/spec | 3835 files · 25624 examples · 1291 passed · 0 failed · 22472 blocked · 1861 skipped · 4.5s |
| `spec/tags/` | one file, `skip.txt`, 7 entries |
| Tags that reach an example | 7 of 7 — verified with `--list` |
| A tag with no reason | dropped in silence |
| A tag naming no example | ignored in silence |
| `spec/tags/README.md` | does not exist |

The 1,861 skips are `ruby_version_is` / `platform_is` guards the harness
evaluates, each already carrying its own reason. Seven of them are tags.

## Decisions

### The format is mspec's own, `tag(comment):description`

The triage on #146 recorded this as the open design question: *"mspec's tag
lines are `fails:<description>` with no reason field, so 'mspec-compatible **and**
carries a reason' is a real design question."* Measured rather than recalled,
against mspec's parser in `spec/mspec/lib/mspec/runner/tag.rb`:

```ruby
m = /^([^()#:]+)(\(([^)]+)?\))?:(.*)$/.match string
@tag, @comment, description = m.values_at(1, 3, 4) if m
```

There **is** a reason field. mspec calls it a comment, it is optional, and
nothing upstream writes one — which is why it does not appear in ruby/spec's own
tag files and why the triage note assumed it was absent. So the question
dissolves: `fails(reason):description` is both mspec-native and carries a
reason, and the Spinel-specific part is only that the reason is *required*.

Run against the real parser to confirm, not to assume:

| line | tag | comment | description |
|---|---|---|---|
| `fails(needs #14):Regexp foo bar` | `"fails"` | `"needs #14"` | `"Regexp foo bar"` |
| `fails:no reason at all` | `"fails"` | `nil` | `"no reason at all"` |
| `fails(a paren m(*a) here):desc` | `nil` | `nil` | `nil` |
| `fails(a colon: fine):desc with (parens)` | `"fails"` | `"a colon: fine"` | `"desc with (parens)"` |

### A parenthesis in a reason is an error, because mspec drops the line in silence

Row three above is the trap. mspec's comment group is `[^)]+`, so the first `)`
ends it, the `:` that should follow is missing, the match fails, and
`read_tags` skips the line without a word — the tag stops existing and the
example silently goes back to failing. Four of the seven reasons being migrated
contain parentheses today.

The reader therefore rejects `(` and `)` in a reason outright. That converts a
silent, delayed breakage into a loud, immediate one, which is the only reason it
is safe to write reasons in a field with a character it cannot hold. Colons and
`#` are fine and are used; the description may contain anything.

### The tag is `fails`, and the report still says `skipped`

`fails` is the tag ruby/spec's tooling excludes on, so a tag file written now
works under mspec after #145 with no configuration beyond the standard
`tags_patterns`. Spinel reports the example `skipped` with its reason, because
the `skipped` column means "not run, and here is why" while `failed` means
"disagreed with Ruby" — and a tag must never be able to manufacture either a
pass or a failure.

Only `fails` is honoured. mspec has `critical`, `slow`, `unstable` and others;
recognising them here would mean a tag that looks live but does nothing, so an
unknown tag is an error rather than a no-op.

### No comment lines in a tag file

`# ...` lines survive mspec — its regex excludes `#` from the tag charset, so
the line parses to a nil tag and is dropped — but they are invisible to every
mspec tool and `mspec tag --purge` rewrites files without them. The prose lives
in `spec/tags/README.md` and in the reason itself. A tag file is nothing but
tags.

### Layout is the path rewrite mspec already documents

From `MSpec.tags_file`:

```
path/to/spec/class/method_spec.rb => path/to/spec/tags/class/method_tags.txt
```

So `spec/ruby/language/regexp/empty_checks_spec.rb` maps to
`spec/tags/language/regexp/empty_checks_tags.txt`. A spec file outside the
corpus — the temp-directory fixtures the harness's own tests build — maps to its
basename under the tags root, which is what makes those tests able to write a
tag at all.

### A stale tag fails the run, checked per file rather than per tree

Every tag loaded for a spec file must match an example discovered in that file.
One that does not is reported and exits non-zero, so a reworded `it` upstream
cannot leave a dead tag behind.

Deliberately **not** built: a check for a `*_tags.txt` whose `*_spec.rb` was
deleted outright. The definition of done asks for "a tag naming an example that
no longer exists", which is the per-example check; the orphan-file case needs a
reverse path mapping and a decision about what a partial run is allowed to
conclude, and no such file exists to motivate either. It is named in *Left for
later* rather than guessed at, which is how #146 got here in the first place.

### `scripts/verify-passes.rb` still does not read tags

Recorded in the #12 triage on this issue and unchanged: verify-passes re-runs
what Spinel *claims*, and a tag is a claim about what Spinel does not claim.
Keeping them independent is what stops a tag from being able to manufacture a
pass. A tagged example is `skipped`, which verify-passes already ignores. No
change is needed and none is made.

## Plan

1. `tags.rs` in the harness: path mapping, mspec-format parser, the three
   validations, unit tests for each.
2. Wire into `main.rs`: per-file load, stale check, a `tag problems` report that
   fails the run. Delete `load_skips`.
3. `--tags DIR` so the harness's own tests can write tag files.
4. Migrate the 7 entries from `skip.txt` to 6 per-spec-file tag files, rewriting
   the four reasons that contain parentheses. Delete `skip.txt`.
5. `spec/tags/README.md`.
6. Integration tests for the four behaviours in the definition of done.
7. Fix `docs/`, `CLAUDE.md` where they now describe the old file.

## Results

| | before | after |
|---|---|---|
| Rust tests | 237 | 254 |
| ruby/spec | 25624 examples · 1291 passed · 0 failed · 22472 blocked · 1861 skipped | unchanged |
| `verify-passes.rb` | 1291 agree | 1291 agree |
| `spec/tags/` | `skip.txt`, 7 entries | 6 `<path>_tags.txt`, 7 tags, `README.md` |
| A tag with no reason | dropped in silence | fails the run |
| A tag naming no example | ignored in silence | fails the run |
| A reason mspec cannot carry | accepted, would vanish after #145 | fails the run |

The counts are deliberately unchanged. This slice moved where the seven skips
are written and what the reader refuses; it did not change which examples run,
which is the check that the migration lost nothing.

### ruby/spec delta

None. No directory newly passes and none regresses; the seven tagged examples
skip exactly as before, with the same reasons reworded.

`CLAUDE.md` makes a spec delta the definition of done for *engine* work, and
this is infra — so the delta being empty is the result rather than a missing
one. A slice that moved the skip mechanism and changed which examples run would
have done two things at once, and the second would have been unreviewable.

### The definition of done

- **`spec/tags/<path>_tags.txt`, laid out the way mspec's own tag files are.**
  Six files, at mspec's documented rewrite of the spec path. Verified by running
  ruby/spec's own vendored `SpecTag` over all seven lines: every one parses to
  `tag="fails"` with a non-empty comment and the right description.
- **A tag carries a reason; a tag without one is an error.** `fails:description`
  is what mspec writes and what mspec accepts. Here it is a `tag problems` entry
  and a non-zero exit.
- **Tagged examples are reported `skipped` with the reason, never `passed`.**
  Unchanged from #158 and still covered, now by a test that reads the reason back
  out of `--list`.
- **A tag naming an example that no longer exists fails the run.** New. Checked
  per spec file on every run, and over the whole corpus by
  `the_whole_corpus_parses_and_reports`, which is the only place a stale tag
  outside `language/` can surface.
- **`spec/tags/README.md`.** States the format, the four rules the reader
  enforces, that a tag is a debt rather than a result, and the three cheaper
  answers to try before writing one.

### Verified by mutation, not just by green

Seven mutations, each expected to turn one test red:

| mutation | test | |
|---|---|---|
| stale tags accepted | `a_tag_naming_no_example_fails_the_run` | caught |
| reasonless tags accepted | `a_tag_without_a_reason_fails_the_run` | caught |
| the balanced-paren check removed | `a_reason_with_a_parenthesis_is_an_error` | caught |
| any tag name honoured | `an_unknown_tag_fails_the_run` | caught |
| the reason not propagated to the example | `a_tagged_example_is_skipped_...` | caught |
| tag problems dropped from the exit code | `a_tag_problem_alone_fails_a_run_...` | caught |
| the tag file never loaded | `a_tagged_example_is_skipped_...` | caught |

The sixth survived the first round. Every tag test until then used a spec file
with a disagreeing example in it, so `1 failed` explained the non-zero exit on
its own and none of them could show the exit code was wired to tag problems at
all. `a_tag_problem_alone_fails_a_run_with_nothing_else_wrong` — a clean spec
file and one stale tag — was written to close it, and is the only test that goes
red for that mutation.

The paren check needed the same treatment in miniature: `fails(m(*a) …)` and
`fails(unbalanced ( …)` are caught by two *other* arms of the parser, so the
`reason.contains('(')` branch was unreached until a third case, `fails(a (b):d`,
was added.

### Reasons rewritten, and why that is the cost of the format

Four of the seven reasons held a parenthesis — `m(*args, &args.pop)`,
`obj.coerce(4.2)`, `defined?(mod.m)`, `(#11)` — and mspec cannot carry one. Each
was reworded to say the same thing without the character; the substance, the
issue numbers and the named next slice all survive. That is the whole price of
the format, it is paid once here, and from now on the reader refuses the
rewording rather than letting anyone discover it after #145.

### Three of the seven reasons named no issue

Writing the README's line "the reason says which slice, by number" made it
checkable, and three of the seven failed it. Two tags said "the rest of the
dialect work" and one said "the coercion protocol" — true, and untracked by any
open issue. Filed, and now named by the tags that wait on them:

| tag | closes with |
|---|---|
| `language/regexp/empty_checks` | #177 — capture-restore records |
| `language/regexp/character_classes` | #178 — the ASCII-only POSIX brackets |
| `core/float/comparison` | #179 — `Numeric#coerce` and coerce-then-retry |

The other four were pointed at the issues that already existed rather than at
the slices that first noticed them: #160 for both `language/send` tags, #161 for
`language/defined`. A reason naming "#11, the calling convention" describes
history; one naming #160 describes work someone can pick up.

### Left for later

- **An orphan `*_tags.txt` whose spec file was deleted.** Not built, argued
  above. It needs a reverse path mapping and a decision about what a partial run
  may conclude from a tag file it did not visit; no such file exists to motivate
  either.
- **Reading mspec's quoted-description escape.** mspec writes
  `"desc with a \n newline"` when a description contains one. No ruby/spec
  description does today; a tag for one would have to be written by hand and
  would be caught by the stale check rather than silently misapplied.

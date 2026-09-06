# `spec/tags/`

A tag is a **debt, not a result.** It records that Spinel does not run a
ruby/spec example and why, so the number nobody argues with — the pass count —
stays honest about what the engine actually does.

`CLAUDE.md` puts it as a rule: *never mark a spec as "expected failure" to make a
slice green; skip it with a reason in `spec/tags/`.* This directory is that
escape hatch, and nothing else is. There are no expected-failure markers in
code.

## Format

One tag per line, in mspec's own format:

```
fails(the reason):the example's full description
```

The description is the name mspec prints: every enclosing `describe`, then the
`it`, joined by spaces. `scripts/spec.sh --list <path>` prints it for every
example, which is where to copy it from.

The file lives at the spec file's own path with `spec/ruby/` swapped for
`spec/tags/` and `_spec.rb` for `_tags.txt` — mspec's documented rewrite:

```
spec/ruby/language/regexp/empty_checks_spec.rb
spec/tags/language/regexp/empty_checks_tags.txt
```

Nothing else belongs in one of these files. Comment lines are not supported:
mspec drops them without a word, and `mspec tag --purge` rewrites files without
them, so a note written there is a note no tool will ever read again.

## What the reader enforces

`spec/harness` reads these files today and mspec reads them after
[#145](https://github.com/ar4mirez/spinel/issues/145). Each of these fails the
run rather than being skipped over, because every one of them is a tag that has
silently stopped skipping anything:

- **A tag needs a reason.** mspec treats the parenthesised part as an optional
  comment and never writes one. Here it is required — a skip nobody wrote a
  reason for is a spec swept under the rug.
- **A reason may not contain a parenthesis.** mspec's tag parser closes the
  reason at the first `)`, fails to match the rest of the line, and drops the tag
  without a word. Rejecting it here turns a silent, delayed breakage into a loud,
  immediate one. Colons, `#` and backticks are fine; the description may contain
  anything.
- **The tag must be `fails`.** mspec also defines `critical`, `slow`, `unstable`
  and others. A tag this harness ignores would look live and do nothing.
- **A tag must name an example that exists.** Upstream rewording one `it` is all
  it takes to leave a tag skipping nothing, so this is checked on every run.

A tagged example is reported `skipped` with its reason — never `passed`, never
`failed`. `scripts/verify-passes.rb` does not read this directory and must not:
it re-runs what Spinel *claims*, and a tag is a claim about what Spinel does not
claim. Keeping the two apart is what stops a tag from being able to manufacture
a pass.

## Writing one

The bar is high on purpose. Before adding a tag, the three cheaper answers:

1. **Fix it.** Five slices in a row found a disagreement small enough to fix in
   the session that revealed it. That is the normal outcome, not the heroic one.
2. **Is it a missing construct?** Then it is already `blocked`, which is a
   separate column and needs no tag. Tags are for examples Spinel *runs* and gets
   wrong.
3. **Is the answer genuinely unknowable yet?** `Error::Unknowable { what, needs }`
   attaches the refusal to the construct rather than to one example, so it cannot
   go stale and covers every example that asks the same question.

A tag is what is left when Spinel runs the example, disagrees with Ruby, and the
fix is a different slice. The reason says which slice, by number.

## What is here now

Twenty-one tags across sixteen files. Every one names the open issue that closes
it: argument evaluation order (#160, twice), method visibility (#161, twice),
fibers (#16, #26), `Kernel#Float` on a String (#181), structural `#hash`
(#21, #22 three times, #23), the `Hash` table and its `inspect` format
(#19, #20, #22 three times), Range-aware indexing in `MatchData` (#33), frozen
string literals (#19), definition hooks (#28), and constant visibility (#185,
four times).

Sixteen of those arrived in one slice, and that is the shape to expect rather
than a lapse. #183 taught the harness to load ruby/spec's fixtures and #157 and
#154 landed the literals that build the values, so several thousand examples ran
for the first time — and an example that has never run cannot have been
disagreeing with Ruby yet. Every one of the sixteen is a subsystem that slice
never touched, which is exactly what "expect this to reveal failures, not only
passes" meant.

That "every one" is not decoration. Three of the original seven said "the
dialect work", "the coercion protocol" or "the restore-record rewrite" and named
no issue, so nothing tracked closing them — #177, #178 and #179 were filed to fix
that, and all three have since been paid off rather than reworded. Their tags are
gone: `language/regexp/character_classes`, `language/regexp/empty_checks` and
`core/float/comparison` are no longer skipped anywhere.

#181 was the one tag added between. It was filed before the tag was written,
which is the order this file asks for — and #185 was filed the same way, before
the four constant-visibility tags that name it.

Each was surfaced by a slice that could not also fix it. The full history is in
the PRD that added this directory, `docs/prd/0020-spec-tags.md`.

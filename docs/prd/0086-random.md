# PRD 0086 — `Random`

Issue: [#249](https://github.com/ar4mirez/spinel/issues/249) · Phase 3 ·
`area:core`

## Objective

`Random`, `Random::Formatter` and `Kernel#rand`/`#srand` behave as CRuby's,
down to the sequence a seed gives: a program seeded with `srand(42)`, or a
spec comparing against a fixed draw, sees CRuby's numbers.

## Design

**MT19937 in Rust, everything else in Ruby.** `crates/spinel-vm/src/mt.rs`
is the reference generator — `init_genrand`, `init_by_array`,
`genrand_int32` — behind two primitives: `__mt_new__(words, single)` answers
a state String, and `__mt_next__(state, count)` draws 32-bit outputs and
writes the advanced state back. The state is a BINARY String each `Random`
holds (624 words and the index), so the default generator belongs to the
heap, and there is no process-global one.

**`core/random.rb`, after `random.c`:** which initialiser a seed's words get,
`genrand_res53` and the inclusive-range real, `limited_rand` and
`limited_big_rand` for an Integer below a limit, `rand_int` and
`rand_range` with CRuby's errors (EDOM for an open or NaN range,
`invalid argument` otherwise), `bytes`, `urandom` and `new_seed` from
`/dev/urandom`, and the private `state`, `left` and `marshal_dump`/`load`
in CRuby's shape. `Kernel#rand` takes the absolute value of a negative
limit, as CRuby does. Sequences were checked against ruby 4.0.6 for seeds
of one, two and many words, Integer, Float and range limits, and `bytes`.

## Results

`core/random`, `Kernel#rand` and `Kernel#srand` all **127** pass. Corpus
23,222 to **23,356** passing, 0 failed; compared example by example with
`main`, 104 blocked and 4 failed examples pass — `core/file` among them,
whose temp names draw from `Random` — and none that passed fails. Every
pass agrees with ruby 4.0.6.

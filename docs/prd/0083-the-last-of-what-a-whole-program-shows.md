# PRD 0083 — the last of what a whole program shows

Issue: [#314](https://github.com/ar4mirez/spinel/issues/314), last slice ·
Phase 3 · `area:cli`, `area:engine`

## Objective

Finish #314: fix what is left that is the CLI's or the compiler's to fix,
and give every other tag it carried the issue that owns it.

## Design

**An Integer literal in a flip-flop is the line number**, as CRuby's
`cond0` makes it: `print if 2..3` under `ruby -n` is `($. == 2)..($. == 3)`.
The literal-in-condition warnings Prism gives for one are not printed for a
program from `-e`, as CRuby's `e_option_supplied` leaves them out.

**`-e` lines keep their bytes.** The arguments are read with `args_os`; the
switches are parsed from their UTF-8 reading, and each `-e` line is taken
from its word's own bytes at the offset the parser recorded, so a line in
Big5 under its magic comment is not mangled on the way in.

**The rest has owners.** RubyGems and `did_you_mean` at boot are #319; a
core key hashed without its `hash` method and the performance warning are
#320; and the others go to #20, #29, #38, #39, #41, #44, #45, #47 and #58.
Two tags that the last slice's changes made pass, `CVE-2011-4815` and the
top-level `return` warning, are deleted.

## Results

Corpus 22,830 to **22,838** passing, 0 failed; none that passed fails. No
tag names #314.

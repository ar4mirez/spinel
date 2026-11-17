# Benchmarks

`scripts/bench.sh` runs everything here. A `bench/*.rb` is a Ruby program timed
on the release `spinel` (its header says how to run it on `ruby --yjit` for the
comparison), and a `bench/*.rs` is a `cargo bench` target. Their numbers go in
the pull request that changes them, not into the repo (`docs/roadmap.md`,
rule 7).

A `bench/*.sh` is a script of its own, run by name (`scripts/bench.sh install`)
and never as part of the run of everything: `install.sh` needs the network.

Three files are the exception, because a script writes them and a check fails
when they drift: `spec-status.md`, from `scripts/spec-status.sh`, and the two
tables below.

## Installing a Rails app's lockfile (#91)

`install/rails/` holds the Gemfile `rails new` writes and the lockfile Bundler
resolves for it. `install.sh` installs it with `spinel install` cold (an empty
store, nothing cached) and warm (every gem stored, a fresh project), and again
with nothing to change, beside `curl` fetching the same `.gem` files and
`bundle install` cold and again; its header says what each case does.

```sh
bench/install.sh           # measure again and rewrite the table (RUNS=5 by default)
bench/install.sh --table   # re-render the table from install/results.tsv
bench/install.sh --check   # fail if the table is not what results.tsv renders to,
                           # the lockfile is not the one measured, or warm is not sub-second
```

Each case's median and fastest wall time are written to `install/results.tsv`,
and the table is rendered from that file; never edit either by hand. A cold
install's time is mostly the network's, so it moves with the connection from
one run to the next; compare it with the `curl` row of the same run. `cargo
test` runs the check.

<!-- install table: written by bench/install.sh; do not edit -->
Spinel at `0fcb55d`, `bench/install/rails/Gemfile.lock`: 121 gems installed, 118 of them fetched (131 MB unpacked); the median and the fastest of 5 runs each.
Bundler: `4.0.20, ruby 4.0.9 (2026-10-23 revision 3e93b23afc) +PRISM [x86_64-linux]`.

| case | median wall | fastest wall | median CPU |
|---|---:|---:|---:|
| curl, the same .gem files | 4.874 s | 3.521 s | 0.272 s |
| spinel install, cold | 8.155 s | 4.515 s | 1.378 s |
| spinel install, warm | 0.261 s | 0.252 s | 0.258 s |
| spinel install, again | 0.288 s | 0.265 s | 0.285 s |
| bundle install, cold | 51.230 s | 39.970 s | 161.773 s |
| bundle install, again | 0.476 s | 0.466 s | 0.471 s |
<!-- end install table -->

## The interpreter's baseline: a yjit-bench subset (#63)

Six benchmarks from [ruby-bench](https://github.com/ruby/ruby-bench) (formerly
yjit-bench), one for each thing an interpreter pays for: method calls (`fib`),
instance-variable reads (`getivar`), keyword arguments (`keyword_args`),
String appends (`str_concat`), Float arithmetic (`nbody`) and allocation
(`binarytrees`). They are copied unmodified into `yjit-bench/`, which says
where from and under what licence. The table is the pre-JIT number that
phase 6's JIT is measured from; the roadmap's target for it is within 1.5× of
`ruby --disable-yjit`.

```sh
scripts/bench.sh --yjit-bench         # measure again and rewrite the table
scripts/bench.sh --yjit-bench-table   # re-render the table from yjit-bench/results.tsv
scripts/bench.sh --yjit-bench-check   # fail if the table is not what results.tsv renders to
```

Every program runs through the same harness on both sides
(`yjit-bench/lib/harness/loader.rb`), pinned to one CPU with address
randomisation off where this machine has working `taskset` and `setarch` (the
line above the table says which applied), and under a 1.5 GB memory cap where
`systemd-run --user` works. The medians are written
to `yjit-bench/results.tsv`, and the table is rendered from that file; never
edit either by hand. The `results.tsv` committed now is one
`scripts/bench.sh --yjit-bench` run's output, started with the 1-minute load
under 2; one run on a shared machine moves by several percent from the next,
so a pull request that changes the table says what its other runs read.
`cargo test` runs the check.

<!-- yjit-bench table: written by scripts/bench.sh --yjit-bench; do not edit -->
Spinel at `012d463`, against `ruby 4.0.9 (2026-10-23 revision 3e93b23afc) +PRISM [x86_64-linux]`. 1 warmup and the median of 5 timed iterations each, one CPU, ASLR off.

| benchmark | Spinel | `ruby --disable-yjit` | Spinel ÷ interpreter | within 1.5× | `ruby --yjit` |
|---|---:|---:|---:|:---:|---:|
| binarytrees | 835.3 ms | 295.8 ms | 2.82× | no | 133.0 ms |
| fib | 382.4 ms | 179.1 ms | 2.14× | no | 23.6 ms |
| getivar | 76.7 ms | 94.9 ms | 0.81× | yes | 89.0 ms |
| keyword_args | 261.6 ms | 208.6 ms | 1.25× | yes | 30.7 ms |
| nbody | 162.6 ms | 88.4 ms | 1.84× | no | 36.2 ms |
| str_concat | 76.9 ms | 71.8 ms | 1.07× | yes | 25.8 ms |
<!-- end yjit-bench table -->

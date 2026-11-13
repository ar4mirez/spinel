# Benchmarks

`scripts/bench.sh` runs everything here. A `bench/*.rb` is a Ruby program timed
on the release `spinel` (its header says how to run it on `ruby --yjit` for the
comparison), and a `bench/*.rs` is a `cargo bench` target. Their numbers go in
the pull request that changes them, not into the repo (`docs/roadmap.md`,
rule 7).

Two files are the exception, because a script writes them and a check fails
when they drift: `spec-status.md`, from `scripts/spec-status.sh`, and the table
below.

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
edit either by hand. The `results.tsv` committed now is not one run's output:
it is the per-cell median of eight `scripts/bench.sh --yjit-bench` runs at
`e7c85f4`, aggregated outside the script, and its `iterations` line says so;
the next `--yjit-bench` run replaces it with that run's medians. `cargo test`
runs the check.

<!-- yjit-bench table: written by scripts/bench.sh --yjit-bench; do not edit -->
Spinel at `e7c85f4`, against `ruby 4.0.9 (2026-10-23 revision 3e93b23afc) +PRISM [x86_64-linux]`. The per-cell median, aggregated outside scripts/bench.sh, of eight scripts/bench.sh --yjit-bench runs, each started with the 1-minute load under 2, of 1 warmup and the median of 5 timed iterations each, one CPU, ASLR off.

| benchmark | Spinel | `ruby --disable-yjit` | Spinel ÷ interpreter | within 1.5× | `ruby --yjit` |
|---|---:|---:|---:|:---:|---:|
| binarytrees | 1015.8 ms | 335.0 ms | 3.03× | no | 171.7 ms |
| fib | 517.3 ms | 195.2 ms | 2.65× | no | 24.7 ms |
| getivar | 139.2 ms | 92.0 ms | 1.51× | no | 95.2 ms |
| keyword_args | 318.0 ms | 205.9 ms | 1.54× | no | 31.6 ms |
| nbody | 248.0 ms | 88.5 ms | 2.80× | no | 38.5 ms |
| str_concat | 103.0 ms | 75.4 ms | 1.37× | yes | 25.9 ms |
<!-- end yjit-bench table -->

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
edit either by hand. `cargo test` runs the check.

<!-- yjit-bench table: written by scripts/bench.sh --yjit-bench; do not edit -->
Spinel at `eef7237`, against `ruby 4.0.9 (2026-10-23 revision 3e93b23afc) +PRISM [x86_64-linux]`. 1 warmup and the median of 5 timed iterations each, one CPU, ASLR off.

| benchmark | Spinel | `ruby --disable-yjit` | Spinel ÷ interpreter | within 1.5× | `ruby --yjit` |
|---|---:|---:|---:|:---:|---:|
| binarytrees | 2336.8 ms | 295.3 ms | 7.91× | no | 145.2 ms |
| fib | 475.5 ms | 186.3 ms | 2.55× | no | 24.0 ms |
| getivar | 137.8 ms | 91.9 ms | 1.50× | yes | 86.3 ms |
| keyword_args | 287.8 ms | 209.0 ms | 1.38× | yes | 31.1 ms |
| nbody | 1254.9 ms | 89.3 ms | 14.05× | no | 43.7 ms |
| str_concat | 96.0 ms | 69.1 ms | 1.39× | yes | 24.0 ms |
<!-- end yjit-bench table -->

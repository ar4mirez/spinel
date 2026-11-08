# A yjit-bench subset

`benchmarks/` is copied unmodified from
[ruby/ruby-bench](https://github.com/ruby/ruby-bench) (formerly
Shopify/yjit-bench) at commit `ad63bf8cac618032adefdd1c5219636a52b47e5e`:

| here | upstream | licence |
|---|---|---|
| `benchmarks/fib.rb` | `benchmarks/fib.rb` | MIT, `LICENSE.md` |
| `benchmarks/getivar.rb` | `benchmarks/getivar.rb` | MIT, `LICENSE.md` |
| `benchmarks/keyword_args.rb` | `benchmarks/keyword_args.rb` | MIT, `LICENSE.md` |
| `benchmarks/str_concat.rb` | `benchmarks/str_concat.rb` | MIT, `LICENSE.md` |
| `benchmarks/nbody/benchmark.rb` | `benchmarks/nbody/benchmark.rb` | Revised BSD, `benchmarks/nbody/LICENSE` |
| `benchmarks/binarytrees/benchmark.rb` | `benchmarks/binarytrees/benchmark.rb` | Revised BSD, `benchmarks/binarytrees/LICENSE` |

`LICENSE.md` is ruby-bench's own. `lib/harness/loader.rb` is not upstream's: it
is Spinel's, at the path the benchmarks `require_relative`, and it is what lets
them run unchanged. To add a benchmark, copy it and its licence from the same
commit (or move every file to a newer one and update the commit above), then
run `scripts/bench.sh --yjit-bench`.

`results.tsv` is written by that script. See `../README.md` for the table.

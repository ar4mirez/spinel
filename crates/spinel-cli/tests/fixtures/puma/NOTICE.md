# Where this fixture's third-party files come from

`tests/puma.rs` runs Puma's own test suite on Spinel (#116) with minitest,
and serves a Sinatra app on Puma, so these are here, unchanged, from their
upstream releases.

## Puma's suite: `app/test/` and `app/puma.gemspec`

The `test/` directory and `puma.gemspec` of [Puma](https://github.com/puma/puma)
at the `v8.1.2` tag, verbatim: the source archive GitHub serves for that tag,
SHA-256 `ed3c9667092b5cefea2615342c355786feaac71805858d82e4dfb9f1ca5dad8a`,
unpacked. Puma's packaged gem does not carry its suite, and the suite's
Bundler fixtures name Puma by its path (`gem "puma", path:`), which needs
the gemspec. `gems/puma-spinel/vendor.sh` vendors them, and
`vendor.sh --check`, which CI's drift job runs, fails when any file differs
from upstream's. That includes the suite's own test certificates and keys
under `app/test/ssl/`, which are Puma's, made for its tests.

Puma is released under the BSD 3-Clause License; its text, with the
copyright holders, is `gems/puma-spinel/LICENSE-puma`, upstream's `LICENSE`.

## The gems the suite and the app run on: `../install/gems/`

The packaged gems rubygems.org serves, each checked against the SHA-256 its
compact index lists (`index/info/`), which `build.rb` fetched, beside the
Sinatra app's, which `../sinatra-lock` brought:

| gem | license |
| --- | --- |
| `minitest-5.27.0.gem` | MIT |
| `minitest-mock-5.27.0.gem` | MIT |
| `minitest-proveit-1.0.1.gem` | MIT |
| `minitest-retry-0.3.1.gem` | MIT |
| `minitest-stub-const-0.6.gem` | MIT |
| `concurrent-ruby-1.3.8.gem` | MIT |
| `localhost-1.8.0.gem` | MIT |
| `bake-0.25.0.gem` | MIT |
| `samovar-2.5.1.gem` | MIT |
| `console-1.38.0.gem` | MIT |
| `fiber-annotation-0.2.0.gem` | MIT |
| `fiber-local-1.1.0.gem` | MIT |
| `fiber-storage-1.0.1.gem` | MIT |

Each `.gem` carries its own license file. They are test data: the test
serves them to `spinel install` from 127.0.0.1, and nothing of them is
built into Spinel.

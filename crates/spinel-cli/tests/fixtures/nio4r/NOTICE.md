# Where this fixture's third-party files come from

`tests/nio4r.rs` runs nio4r's own test suite on Spinel (#115) with RSpec, so
both are here, unchanged, from their upstream releases.

## nio4r's suite: `app/spec/`

The `spec/` directory of [nio4r](https://github.com/socketry/nio4r) at the
`v2.7.5` tag, verbatim: the source archive GitHub serves for that tag,
SHA-256 `6b17580f7a84903b5c76a97bb926098cfeb012fd6189a0481f417df6d944d93f`,
unpacked. nio4r's packaged gem does not carry its suite.
`gems/nio4r-spinel/vendor.sh` vendors it, and `vendor.sh --check`, which
CI's drift job runs, fails when any file differs from upstream's.

nio4r is released under the MIT License; its text, with the copyright
holders, is `gems/nio4r-spinel/LICENSE-nio4r`, upstream's `license.md`.

## RSpec: `../install/gems/`

The packaged gems rubygems.org serves, each checked against the SHA-256 its
compact index lists (`index/info/`), which `build.rb` fetched:

| gem | license |
| --- | --- |
| `rspec-core-3.13.7.gem` | MIT |
| `rspec-expectations-3.13.6.gem` | MIT |
| `rspec-mocks-3.13.9.gem` | MIT |
| `rspec-support-3.13.8.gem` | MIT |
| `diff-lcs-1.6.2.gem` | MIT, Artistic-1.0-Perl or GPL-2.0-or-later, at the user's choice |

Each `.gem` carries its own license file. They are test data: the test
serves them to `spinel install` from 127.0.0.1, and nothing of them is
built into Spinel.

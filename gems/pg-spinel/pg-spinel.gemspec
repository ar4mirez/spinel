# The pg gem for Spinel (#114, docs/package-manager.md, section 6, step 2):
# upstream pg 1.7.0's Ruby API, its `lib/` unchanged, its C half in Ruby
# (`lib/pg_ext.rb` and `lib/pg_ext/`) over a `.spinel` extension built with
# `spinel-ext` (`ext/pg_spinel`), linked with the system's libpq.
#
# `spinel install` installs it in place of a locked `pg 1.7.0`; its version
# is that version and a revision. Like a precompiled gem it ships its
# extension built and lists no `extensions`, so `build.rb`, which builds the
# extension and the `.gem` for this machine's platform, sets `platform` and
# adds the library to `files`.
Gem::Specification.new do |s|
  s.name = "pg-spinel"
  s.version = "1.7.0.1"
  s.summary = "The pg gem's Ruby API for Spinel, on a spinel-ext extension over libpq"
  s.description = "Spinel's substitute for pg 1.7.0: `spinel install` installs it in place " \
                  "of a locked pg 1.7.0, and `require \"pg\"` loads it. It runs only on Spinel, " \
                  "and needs the system's libpq."
  s.authors = ["Spinel"]
  s.licenses = ["MIT", "BSD-2-Clause"]
  s.homepage = "https://github.com/ar4mirez/spinel"
  s.metadata = {"source_code_uri" => "https://github.com/ar4mirez/spinel/tree/main/gems/pg-spinel"}
  s.files = Dir.chdir(__dir__) { Dir["lib/**/*.rb"].sort } + ["LICENSE-pg", "BSDL-pg"]
  s.require_paths = ["lib"]
  # Upstream's floor; Spinel is Ruby 4.0.
  s.required_ruby_version = ">= 3.1"
end

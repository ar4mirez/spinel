# The sqlite3 gem for Spinel (#113, docs/package-manager.md, section 6, step
# 2): upstream sqlite3 2.10.2's Ruby API, its `lib/` unchanged, its C half
# in Ruby (`lib/sqlite3/sqlite3_native.rb`) over a `.spinel` extension built
# with `spinel-ext` (`ext/sqlite3_spinel`), SQLite 3.54.1 compiled in.
#
# `spinel install` installs it in place of a locked `sqlite3 2.10.2`; its
# version is that version and a revision. Like a precompiled gem it ships its
# extension built and lists no `extensions`, so `build.rb`, which builds the
# extension and the `.gem` for this machine's platform, sets `platform` and
# adds the library to `files`.
Gem::Specification.new do |s|
  s.name = "sqlite3-spinel"
  s.version = "2.10.2.1"
  s.summary = "The sqlite3 gem's Ruby API for Spinel, on a spinel-ext extension"
  s.description = "Spinel's substitute for sqlite3 2.10.2: `spinel install` installs it in place " \
                  "of a locked sqlite3 2.10.2, and `require \"sqlite3\"` loads it. It runs only on Spinel."
  s.authors = ["Spinel"]
  s.licenses = ["MIT", "BSD-3-Clause"]
  s.homepage = "https://github.com/ar4mirez/spinel"
  s.metadata = {"source_code_uri" => "https://github.com/ar4mirez/spinel/tree/main/gems/sqlite3-spinel"}
  s.files = Dir.chdir(__dir__) { Dir["lib/**/*.rb"].sort } + ["LICENSE-sqlite3-ruby"]
  s.require_paths = ["lib"]
  # Upstream's floor; Spinel is Ruby 4.0.
  s.required_ruby_version = ">= 3.2"
end

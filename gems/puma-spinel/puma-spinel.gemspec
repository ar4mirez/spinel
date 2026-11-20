# The puma gem for Spinel (#116, docs/package-manager.md, section 6, step
# 2): upstream Puma 8.1.2's Ruby API and executables, its `lib/` and `bin/`
# unchanged, its C half in Ruby (`lib/puma/puma_http11.rb`) over a `.spinel`
# extension built with `spinel-ext` (`ext/puma_spinel`): the HTTP parser,
# Ragel's machine of upstream's grammar, and MiniSSL's calls on the
# system's OpenSSL 3.
#
# `spinel install` installs it in place of a locked `puma 8.1.2`; its
# version is that version and a revision, and it needs what upstream needs,
# nio4r. Like a precompiled gem it ships its extension built and lists no
# `extensions`, so `build.rb`, which builds the extension and the `.gem` for
# this machine's platform, sets `platform` and adds the library to `files`.
Gem::Specification.new do |s|
  s.name = "puma-spinel"
  s.version = "8.1.2.1"
  s.summary = "The puma gem's Ruby API for Spinel, on a spinel-ext extension"
  s.description = "Spinel's substitute for puma 8.1.2: `spinel install` installs it in place " \
                  "of a locked puma 8.1.2, and `require \"puma\"` and `puma` run it. It runs only " \
                  "on Spinel."
  s.authors = ["Spinel"]
  s.licenses = ["MIT", "BSD-3-Clause"]
  s.homepage = "https://github.com/ar4mirez/spinel"
  s.metadata = {"source_code_uri" => "https://github.com/ar4mirez/spinel/tree/main/gems/puma-spinel"}
  s.files = Dir.chdir(__dir__) { Dir["lib/**/*.rb"].sort + Dir["bin/*"].sort } + ["LICENSE-puma"]
  s.bindir = "bin"
  s.executables = ["puma", "pumactl"]
  s.require_paths = ["lib"]
  s.add_runtime_dependency "nio4r", "~> 2.0"
  # Upstream's floor; Spinel is Ruby 4.0.
  s.required_ruby_version = ">= 3.0"
end

# The nio4r gem for Spinel (#115, docs/package-manager.md, section 6, step
# 2): upstream nio4r 2.7.5's Ruby API, its `lib/` unchanged, its C half in
# Ruby (`lib/nio4r_ext.rb`) over a `.spinel` extension built with
# `spinel-ext` (`ext/nio4r_spinel`) that makes the epoll or kqueue calls
# upstream has libev make.
#
# `spinel install` installs it in place of a locked `nio4r 2.7.5`; its
# version is that version and a revision. Like a precompiled gem it ships its
# extension built and lists no `extensions`, so `build.rb`, which builds the
# extension and the `.gem` for this machine's platform, sets `platform` and
# adds the library to `files`.
Gem::Specification.new do |s|
  s.name = "nio4r-spinel"
  s.version = "2.7.5.1"
  s.summary = "The nio4r gem's Ruby API for Spinel, on a spinel-ext extension"
  s.description = "Spinel's substitute for nio4r 2.7.5: `spinel install` installs it in place " \
                  "of a locked nio4r 2.7.5, and `require \"nio\"` loads it. It runs only on Spinel."
  s.authors = ["Spinel"]
  s.licenses = ["MIT"]
  s.homepage = "https://github.com/ar4mirez/spinel"
  s.metadata = {"source_code_uri" => "https://github.com/ar4mirez/spinel/tree/main/gems/nio4r-spinel"}
  s.files = Dir.chdir(__dir__) { Dir["lib/**/*.rb"].sort } + ["LICENSE-nio4r"]
  s.require_paths = ["lib"]
  # Upstream's floor; Spinel is Ruby 4.0.
  s.required_ruby_version = ">= 2.6"
end

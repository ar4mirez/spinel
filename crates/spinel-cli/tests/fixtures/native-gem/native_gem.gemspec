# The example gem of the native-extension loader (#104): a Ruby half in
# `lib/native_gem.rb` over a `.spinel` extension built from
# `ext/native_gem/` with `spinel-ext`.
#
# Like a precompiled native gem, it ships the extension built and lists no
# `extensions`: nothing is compiled at install (docs/package-manager.md,
# section 6). `tests/native_gem.rs` builds the crate with cargo, puts the
# library at `lib/native_gem/native_gem.spinel` in a copy of this directory,
# and installs that copy as a path gem.
Gem::Specification.new do |s|
  s.name = "native_gem"
  s.version = "0.1.0"
  s.summary = "A gem with a Spinel native extension"
  s.authors = ["Spinel"]
  s.files = ["lib/native_gem.rb", "lib/native_gem/version.rb", "lib/native_gem/native_gem.spinel"]
end

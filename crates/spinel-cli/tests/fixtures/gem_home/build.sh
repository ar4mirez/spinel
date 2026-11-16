#!/usr/bin/env bash
#
# Regenerate what `gem install` makes of the gem-store fixtures (#82), the
# GEM_HOME `spinel fetch --gem-home` must match:
#
#   crates/spinel-cli/tests/fixtures/gem_home/build.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/gem_home
#
# The three `.gem`s of `../gem_store/gems/` are installed into a fresh
# GEM_HOME by `Gem::Installer`, run by CRuby (which has Psych) on the
# RubyGems Spinel ships in `stdlib/`, as `gem install --env-shebang
# --ignore-dependencies --no-document` would install them on Spinel, where
# `RbConfig::CONFIG["ruby_install_name"]` is `spinel` (docs/cli.md). Kept:
#
# - `tree.txt`, every path in the GEM_HOME, a directory with a trailing `/`;
# - `specifications/` and `bin/`, the files `gem install` writes itself.
#
# - `bundler-stub`, the stub RubyGems writes for an executable of the gem
#   `bundler`, which differs, named `it's\x` to show its escaping.
#
# `gems/` is the gems' own files, which the store test already checks, and
# `cache/<gem>.gem` is the one thing Spinel does not keep, so `tree.txt`
# leaves those files out.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
stdlib="$here/../../../../../stdlib"
work="$(mktemp -d "${TMPDIR:-/tmp}/spinel-gem-home.XXXXXX")"
trap 'rm -rf "$work"' EXIT

ruby --disable-gems -I "$stdlib" -e '
  require "rubygems"
  require "rubygems/installer"
  abort "stdlib/ has RubyGems #{Gem::VERSION}; is the -I path right?" unless
    File.expand_path(Gem.method(:dir).source_location[0]).start_with?(File.expand_path(ARGV[0]))
  RbConfig::CONFIG["ruby_install_name"] = "spinel"
  ARGV[2..].each do |gem|
    Gem::Installer.at(gem, install_dir: ARGV[1], env_shebang: true,
                      ignore_dependencies: true, wrappers: true, document: []).install
  end
' "$stdlib" "$work/gem_home" "$here"/../gem_store/gems/*.gem

rm -rf "$here/specifications" "$here/bin"
cp -R "$work/gem_home/specifications" "$work/gem_home/bin" "$here/"
(cd "$work/gem_home" && find . -mindepth 1 | while read -r path; do
  if [[ -d "$path" ]]; then echo "${path#./}/"; else echo "${path#./}"; fi
done | grep -v -e '^cache/.' -e '^gems/[^/]*/.' | LC_ALL=C sort) > "$here/tree.txt"

ruby --disable-gems -I "$stdlib" -e '
  require "rubygems"
  require "rubygems/installer"
  # The shebang is read from the executable, which this gem does not have.
  Gem::Installer.prepend(Module.new { def shebang(_) = "#!/usr/bin/env spinel" })
  spec = Gem::Specification.new { |s| s.name = "bundler"; s.version = "4.0.16" }
  print Gem::Installer.for_spec(spec, env_shebang: true).app_script_text(%q(it'"'"'s\x))
' > "$here/bundler-stub"

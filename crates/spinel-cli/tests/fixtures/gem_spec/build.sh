#!/usr/bin/env bash
#
# Regenerate the expected installed gemspecs (#82) from the YAML beside them:
#
#   crates/spinel-cli/tests/fixtures/gem_spec/build.sh
#   git diff --exit-code crates/spinel-cli/tests/fixtures/gem_spec
#
# Each `yaml/<case>.yaml` is a gem's `metadata` as a store entry keeps it:
# some written by hand to reach corners of the format (anchors, `!binary`,
# RubyGems 1.3's fields, a platform object, escapes), some copied from real
# gems. `gemspec/<case>.gemspec` is what `gem install` would write for it:
# `Gem::Specification.from_yaml`, `installed_by_version` set, and
# `to_ruby_for_cache`, run by CRuby (which has Psych) on the RubyGems Spinel
# ships in `stdlib/`, so the RubyGems is the one Spinel's port follows.
# `SOURCE_DATE_EPOCH` fixes the date of a gemspec that has none.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
stdlib="$here/../../../../../stdlib"
mkdir -p "$here/gemspec"
rm -f "$here"/gemspec/*.gemspec
SOURCE_DATE_EPOCH=1700000000 ruby --disable-gems -I "$stdlib" -e '
  require "rubygems"
  abort "stdlib/ has RubyGems #{Gem::VERSION}; is the -I path right?" unless
    File.expand_path(Gem.method(:dir).source_location[0]).start_with?(File.expand_path(ARGV[0]))
  Dir[File.join(ARGV[1], "*.yaml")].sort.each do |file|
    spec = Gem::Specification.from_yaml(File.read(file))
    spec.installed_by_version = Gem.rubygems_version
    out = File.join(ARGV[2], File.basename(file, ".yaml") + ".gemspec")
    File.binwrite(out, spec.to_ruby_for_cache)
  end
' "$stdlib" "$here/yaml" "$here/gemspec"

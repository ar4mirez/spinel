# mspec configuration for running ruby/spec on Spinel (#145).
#
#   spinel run spec/mspec/bin/mspec-run -B spec/spinel.mspec core/array
#
# `scripts/spec.sh` is the usual way in. This file is loaded by mspec itself,
# on Spinel, while it reads its options.

class MSpecScript
  # Paths on the command line are relative to the ruby/spec checkout, as they
  # were for `spec/harness`.
  set :prefix, File.expand_path("ruby", __dir__)

  # `spec/tags/<path>_tags.txt` mirrors `spec/ruby/<path>_spec.rb`; see
  # `spec/tags/README.md`. A `fails` tag is a skip, never a pass.
  set :tags_patterns, [
    [%r{spec/ruby/}, "spec/tags/"],
    [/_spec\.rb$/, "_tags.txt"],
  ]
  set :xtags, ["fails"]

  # `SPINEL_SPEC_TAGGED=1` runs only the tagged examples instead: one that
  # passes is a tag to delete.
  if ENV["SPINEL_SPEC_TAGGED"]
    set :xtags, []
    set :tags, ["fails"]
  end
end

require_relative "spinel/runner"
MSpecScript.set :formatter, SpinelFormatter

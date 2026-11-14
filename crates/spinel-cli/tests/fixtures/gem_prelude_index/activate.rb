# Run as `ruby -I lib activate.rb`. A gem activated by RubyGems goes into
# `$LOAD_PATH` at `Gem.load_path_insert_index`: the first entry the
# interpreter marked with `@gem_prelude_index`, which is `sitelibdir`. So it
# lands after the `-I` entries, and an app's own file still wins.
here = __dir__
app_lib = File.join(here, "lib")
site = RbConfig::CONFIG["sitelibdir"]

p $LOAD_PATH.first.equal?($LOAD_PATH.find { |path| File.realpath(path) == app_lib rescue false })
p $LOAD_PATH.take_while { |path| !path.instance_variable_defined?(:@gem_prelude_index) }.size
p Gem.load_path_insert_index == $LOAD_PATH.index(site)

spec = Gem::Specification.new do |s|
  s.name = "greet"
  s.version = "1.0"
  s.summary = "a fixture"
  s.authors = ["spinel"]
  s.files = ["lib/greet.rb", "lib/shared.rb"]
end
spec.loaded_from = File.join(here, "specifications", "greet-1.0.gemspec")
p spec.activate

gem_lib = File.join(here, "gems", "greet-1.0", "lib")
at = $LOAD_PATH.index(gem_lib)
p at == 1
p at < $LOAD_PATH.index(site)
p $LOAD_PATH[at].instance_variable_defined?(:@gem_prelude_index)
p Gem.load_path_insert_index == $LOAD_PATH.index(site)
p $LOAD_PATH.resolve_feature_path("greet") == [:rb, File.join(gem_lib, "greet.rb")]
require "greet"
puts Greet::WHO
require "shared"
p SHARED

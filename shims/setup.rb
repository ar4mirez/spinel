# Spinel's setup shim (docs/cli.md, "What `spinel run` sets up").
#
# `spinel run` and `spinel <file>` run this before the program and before any
# file it requires. `ruby`'s own command line (`spinel -e`, `spinel -w app.rb`)
# does not, as `ruby` looks for no project.
#
# 1. The project's gems. The nearest `.spinel/gem_home/` from the current
#    directory upward is GEM_HOME and the whole of GEM_PATH, as `bundle exec`
#    sets them for a `BUNDLE_PATH`, and its `bin/` goes first on PATH. They
#    are set in the environment, so a child process — a gem's executable, a
#    nested `spinel` — finds the same gems and the same executables. RubyGems
#    reads GEM_HOME and GEM_PATH when it first needs its paths; a `Gem`
#    already loaded is told with `Gem.paths=`.
#    ponytail: RubyGems is not loaded at boot yet (#319), so today it is the
#    environment that carries the gem home; `Gem.paths=` starts mattering then.
# 2. `.spinel/require_map.bin`: not read yet (#66).
# 3. The bytecode cache, `.spinel/bytecode/` beside the gem home: there is no
#    cache to register yet (#40), which reads it from here when it lands.
#
# Outside a project nothing changes, and that is not an error.

start =
  begin
    Dir.pwd
  rescue SystemCallError
    nil
  end

gem_home = nil
dir = start
while dir
  candidate = File.join(dir, ".spinel", "gem_home")
  if File.directory?(candidate)
    gem_home = candidate
    break
  end
  parent = File.dirname(dir)
  dir = parent == dir ? nil : parent
end

if gem_home
  ENV["GEM_HOME"] = gem_home
  ENV["GEM_PATH"] = gem_home
  # First, and only once, so a `spinel` run from a `spinel` does not grow it.
  # A limit of -1 keeps a trailing empty entry, which means the current
  # directory.
  bin = File.join(gem_home, "bin")
  rest = ENV.fetch("PATH", "").split(File::PATH_SEPARATOR, -1).reject { |entry| entry == bin }
  ENV["PATH"] = [bin, *rest].join(File::PATH_SEPARATOR)
  if defined?(Gem) && Gem.respond_to?(:paths=)
    Gem.paths = { "GEM_HOME" => gem_home, "GEM_PATH" => gem_home }
  end
end

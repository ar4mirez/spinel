# Spinel's gemspec evaluator (docs/package-manager.md, "Git and path
# sources"): the gemspecs of git and path sources, evaluated as Bundler 4's
# `Source::Path#load_spec_files` evaluates them, printed as JSON.
#
# `spinel install` runs it on the VM in a child `spinel`, with
# `$spinel_gemspec_sources` set in front of it to an Array of Hashes:
#
#   {"dir" => "/abs/checkout", "glob" => "{,*,*/*}.gemspec", "serialize" => false}
#
# For each source, every file the glob finds under `dir` is evaluated as
# `Bundler.load_gemspec` evaluates one: from its own directory, against a
# copy of the top-level binding, its `loaded_from` the file, then validated
# as `Bundler.rubygems.validate` validates it. Deeper files first, so a
# shallower gemspec of the same gem wins, as `Index#<<` keeps the later one.
# With `serialize`, each file is then overwritten with `to_ruby`, as
# `Source::Git#serialize_gemspecs_in` leaves a checkout, so what reads the
# checkout later reads the evaluated gemspec and runs nothing of the gem's.
#
# The result is one Array per source, of one Hash per gem, on stdout. An
# error is Bundler's message on stderr, and the exit status is 1.

require "json"

def spinel_load_gemspec(file)
  contents = File.read(file)
  spec =
    if contents.start_with?("---")
      Gem::Specification.from_yaml(contents)
    else
      Dir.chdir(File.dirname(file)) do
        eval(contents, TOPLEVEL_BINDING.dup, file)
      end
    end
  unless spec.is_a?(Gem::Specification)
    raise "#{File.basename(file)} did not evaluate to a Gem::Specification"
  end
  spec.loaded_from = file
  spec
rescue ScriptError, StandardError => e
  warn "There was an error while loading `#{File.basename(file)}`: #{e.message}"
  exit 1
end

def spinel_validate(spec)
  spec.validate(false)
rescue Gem::InvalidSpecificationException => e
  warn "The gemspec at #{spec.loaded_from} is not valid. Please fix this gemspec.\n" \
    "The validation error was '#{e.message}'"
  exit 1
end

def spinel_dependencies(deps)
  deps.map { |d| [d.name, d.requirement.as_list] }
end

result = $spinel_gemspec_sources.map do |source|
  dir = source["dir"]
  files = Dir.glob(source["glob"], base: dir).map { |f| File.expand_path(f, dir) }
  files = files.sort_by { |f| -f.split(File::SEPARATOR).size }
  found = {}
  files.each do |file|
    spec = spinel_load_gemspec(file)
    spinel_validate(spec)
    spec.installed_by_version = Gem::VERSION
    File.binwrite(file, spec.to_ruby) if source["serialize"]
    found[[spec.name, spec.version.to_s, spec.platform.to_s]] = {
      "file" => file,
      "dir" => File.dirname(file),
      "name" => spec.name,
      "version" => spec.version.to_s,
      "platform" => spec.platform.to_s,
      "dependencies" => spinel_dependencies(spec.runtime_dependencies),
      "development_dependencies" => spinel_dependencies(spec.development_dependencies),
      "required_ruby_version" => spec.required_ruby_version.as_list,
      "required_rubygems_version" => spec.required_rubygems_version.as_list,
      "require_paths" => spec.require_paths,
      "bindir" => spec.bindir,
      "executables" => spec.executables,
      "extensions" => spec.extensions,
      "gemspec" => spec.to_ruby_for_cache,
    }
  end
  found.values
end

puts JSON.generate(result)

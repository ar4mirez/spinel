# Loaded into `bundle lock` by `bundler.sh` (#78), so Bundler resolves as
# Spinel would on the case's Ruby: it makes Bundler's idea of the running Ruby
# (`Ruby\0`), RubyGems (`RubyGems\0`) and local platform the case's, and
# changes nothing else about how Bundler resolves. Each value is one
# Bundler or RubyGems reads in exactly one place.

require "bundler"

ruby = ENV.fetch("ORACLE_RUBY")
rubygems = ENV.fetch("ORACLE_RUBYGEMS")
platform = Gem::Platform.new(ENV.fetch("ORACLE_PLATFORM"))

# `Ruby\0`'s one version, and the root's requirement on it.
Bundler::RubyVersion.define_singleton_method(:system) do
  @oracle_system ||= Bundler::RubyVersion.new(ruby, nil, "ruby", ruby)
end

# `RubyGems\0`'s one version, and the root's requirement on it.
Bundler::Definition.prepend(Module.new do
  define_method(:metadata_dependencies) do
    @metadata_dependencies ||= [
      Bundler::Dependency.new("Ruby\0", Bundler::RubyVersion.system.gem_version),
      Bundler::Dependency.new("RubyGems\0", rubygems),
    ]
  end
end)

# `Source::Metadata#specs` with `Gem::VERSION` replaced: the rest is
# Bundler's, so `bundler` is still the one running.
Bundler::Source::Metadata.prepend(Module.new do
  define_method(:specs) do
    @specs ||= begin
      original = super()
      Bundler::Index.build do |index|
        index << Gem::Specification.new("Ruby\0", Bundler::RubyVersion.system.gem_version)
        index << Gem::Specification.new("RubyGems\0", rubygems) {|s| s.required_rubygems_version = Gem::Requirement.default }
        original.search("bundler").each {|spec| index << spec }
        index.each {|spec| spec.source = self }
      end
    end
  end
end)

Gem::Platform.define_singleton_method(:local) {|*| platform }

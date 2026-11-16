# -*- encoding: utf-8 -*-
# stub: legacy 0.1.0 ruby lib

Gem::Specification.new do |s|
  s.name = "legacy".freeze
  s.version = "0.1.0".freeze

  s.required_rubygems_version = Gem::Requirement.new(">= 0".freeze) if s.respond_to? :required_rubygems_version=
  s.require_paths = ["lib".freeze]
  s.authors = ["Old Timer".freeze]
  s.autorequire = "legacy".freeze
  s.date = "2010-07-04"
  s.description = "Old gems wrote some strings as binary: \xC3\xA9".freeze
  s.email = "old@example.com".freeze
  s.executables = ["legacy".freeze]
  s.extra_rdoc_files = ["README".freeze]
  s.files = ["README".freeze, "bin/legacy".freeze]
  s.homepage = "http://legacy.example.com".freeze
  s.rdoc_options = ["--charset=UTF-8".freeze]
  s.rubygems_version = "1.3.7".freeze
  s.summary = "A gem as RubyGems 1.3 wrote one".freeze

  s.installed_by_version = "4.0.16".freeze

  s.specification_version = 3

  s.add_runtime_dependency(%q<json>.freeze, [">= 0".freeze])
  s.add_development_dependency(%q<rspec>.freeze, ["= 1.3.0".freeze])
end

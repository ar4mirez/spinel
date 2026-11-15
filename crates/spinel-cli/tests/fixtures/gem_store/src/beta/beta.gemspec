Gem::Specification.new do |s|
  s.name = "beta"
  s.version = "2.1.0"
  s.summary = "A gem-store fixture with a symlink that stays inside the gem."
  s.authors = ["Spinel"]
  s.license = "MIT"
  s.files = ["lib/beta.rb", "lib/beta/version.rb", "lib/beta/current.rb"]
  s.add_dependency "alpha", "~> 1.0"
end

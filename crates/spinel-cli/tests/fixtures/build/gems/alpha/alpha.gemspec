Gem::Specification.new do |s|
  s.name = "alpha"
  s.version = "1.0.0"
  s.summary = "A gem spinel build packs"
  s.authors = ["Spinel"]
  s.files = ["lib/alpha.rb", "exe/alpha"]
  s.bindir = "exe"
  s.executables = ["alpha"]
  s.add_dependency "gamma"
end

Gem::Specification.new do |s|
  s.name = "pair"
  s.version = "1.0.0"
  s.summary = "A `spinel x` fixture: two executables, neither named like the gem."
  s.authors = ["Spinel"]
  s.license = "MIT"
  s.files = ["lib/pair.rb", "exe/pair-a", "exe/pair-b"]
  s.bindir = "exe"
  s.executables = ["pair-a", "pair-b"]
end

Gem::Specification.new do |s|
  s.name = "greet"
  s.version = "2.0.0"
  s.summary = "A `spinel x` fixture: prints what it was run with."
  s.authors = ["Spinel"]
  s.license = "MIT"
  s.files = ["lib/greet.rb", "exe/greet"]
  s.bindir = "exe"
  s.executables = ["greet"]
  s.add_dependency "shout", "~> 1.0"
end

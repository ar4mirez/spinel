Gem::Specification.new do |s|
  s.name = "respawn"
  s.version = "1.0.0"
  s.summary = "A bin stub fixture (#70): starts the current Ruby again, as a gem that runs itself in a subprocess does."
  s.authors = ["Spinel"]
  s.license = "MIT"
  s.files = ["lib/respawn.rb", "exe/respawn"]
  s.bindir = "exe"
  s.executables = ["respawn"]
end

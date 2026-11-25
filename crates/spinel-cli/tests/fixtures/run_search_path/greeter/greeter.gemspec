Gem::Specification.new do |s|
  s.name = "greeter"
  s.version = "0.1.0"
  s.summary = "A gem with an executable, read from a path"
  s.authors = ["Spinel"]
  s.files = ["lib/greeter.rb", "exe/greeter"]
  s.bindir = "exe"
  s.executables = ["greeter"]
end

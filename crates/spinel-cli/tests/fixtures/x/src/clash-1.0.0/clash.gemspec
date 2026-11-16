Gem::Specification.new do |s|
  s.name = "clash"
  s.version = "1.0.0"
  s.summary = "A `spinel x` fixture: its executable's name is a dependency's too."
  s.authors = ["Spinel"]
  s.license = "MIT"
  s.files = ["lib/clash.rb", "exe/clash"]
  s.bindir = "exe"
  s.executables = ["clash"]
  s.add_dependency "clash-dep", "~> 1.0"
end

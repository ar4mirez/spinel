Gem::Specification.new do |s|
  s.name = "clash-dep"
  s.version = "1.0.0"
  s.summary = "A `spinel x` fixture: an executable named like the gem that depends on it."
  s.authors = ["Spinel"]
  s.license = "MIT"
  s.files = ["lib/clash_dep.rb", "exe/clash"]
  s.bindir = "exe"
  s.executables = ["clash"]
end

# Evaluated from its own directory, as Bundler evaluates it: the version
# comes from a file beside it, and the files from `git ls-files`.
require_relative "lib/widget/version"

Gem::Specification.new do |s|
  s.name = "widget"
  s.version = Widget::VERSION
  s.summary = "A gem from a git repository"
  s.authors = ["Spinel"]
  s.files = `git ls-files -z`.split("\x00")
  s.add_dependency "shared", ">= 1.0"
end

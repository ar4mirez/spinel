# Probed by `tests/ruby_flags.rs`: RubyGems is loaded before a `-r` file.
p defined?(Gem::Specification)

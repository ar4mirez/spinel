# `Object.const_get("Baz::Bar")`: past the first segment the lookup is a
# qualified one, which does not reach `Object` — not its constants, and not
# the autoloads registered on it. CRuby 4.0.9 raises NameError.
autoload :Bar, File.expand_path("bar.rb", __dir__)
class Baz; end
begin
  Object.const_get("Baz::Bar")
  puts "found"
rescue NameError => e
  puts "NameError: #{e.message}"
end

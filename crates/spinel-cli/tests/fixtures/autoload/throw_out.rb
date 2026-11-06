# A `throw` out of an autoloaded file unwinds without an exception. The load
# did not finish, so nothing about it is left marked: another thread reading
# the name runs the autoload again rather than waiting on a load that is gone.
# CRuby 4.0.9 prints :thrown.
autoload :Thrown, File.expand_path("thrown.rb", __dir__)
catch(:out) { Thrown }
reader = Thread.new { Thrown }
p reader.join(5) ? reader.value : :hung

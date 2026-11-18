# Writes cases.out, the known answers tests/bigdecimal.rs checks the
# built-in bigdecimal (#109) against: what cases.rb prints on CRuby, with
# CRuby's own bigdecimal. The file checked in was made by ruby 4.0.9
# (bigdecimal 4.0.1, the release Ruby 4.0 bundles):
#
#   ruby crates/spinel-cli/tests/fixtures/bigdecimal/make.rb
require "rbconfig"

abort "make.rb wants CRuby, not #{RUBY_ENGINE}" unless RUBY_ENGINE == "ruby"

out = IO.popen([RbConfig.ruby, File.join(__dir__, "cases.rb")], &:read)
abort "cases.rb failed on CRuby" unless $?.success?
File.write(File.join(__dir__, "cases.out"), out)

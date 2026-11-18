# Writes the fixtures `tests/psych.rs` checks the built-in psych (#108)
# against, with CRuby's own Psych: what YAML.dump and dump_stream make of
# each case in cases.rb, what a parsed document emits again (yaml/), and
# what roundtrip.rb prints on CRuby (roundtrip.out). The files checked in
# were made by ruby 4.0.9 (Psych 5.3.1, libyaml 0.2.5):
#
#   ruby crates/spinel-cli/tests/fixtures/psych/make.rb
require "rbconfig"
require_relative "cases"

abort "make.rb wants CRuby, not #{RUBY_ENGINE}" unless RUBY_ENGINE == "ruby"

dir = File.join(__dir__, "yaml")
Dir.mkdir(dir) unless Dir.exist?(dir)

PsychCases::DUMPS.each do |name, object, options|
  yaml = options ? YAML.dump(object, **options) : YAML.dump(object)
  File.binwrite(File.join(dir, "#{name}.yaml"), yaml)
end

PsychCases::STREAMS.each do |name, objects|
  File.binwrite(File.join(dir, "#{name}.yaml"), YAML.dump_stream(*objects))
end

PsychCases::REEMIT.each do |name, source|
  File.binwrite(File.join(dir, "reemit_#{name}.yaml"), Psych.parse_stream(source).yaml)
end

out = IO.popen([RbConfig.ruby, File.join(__dir__, "roundtrip.rb")], &:read)
abort "roundtrip.rb failed on CRuby" unless $?.success?
File.write(File.join(__dir__, "roundtrip.out"), out)

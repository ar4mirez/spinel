# mspec configuration for `scripts/verify-passes.rb` (#145): the corpus on a
# real Ruby, running only the examples Spinel passed. See `spec/spinel/verify.rb`.

class MSpecScript
  set :prefix, File.expand_path("ruby", __dir__)
end

require_relative "spinel/verify"
MSpec.register_mode :report_on
SpecGuard.guards << :ruby_bug
MSpecScript.set :formatter, SpinelVerifyFormatter

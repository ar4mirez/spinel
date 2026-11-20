# Loaded into `bundle lock` by `relax_compare.py` (#816), after
# `bundler_oracle.rb`: logs what each try's `Resolver#find_names_to_relax`
# returns to $RELAX_LOG, one JSON line per try (`unlock`, the gems to unlock,
# and `pre`, those to let prereleases in for), and changes nothing about how
# Bundler resolves.
require "json"

module RelaxHook
  def find_names_to_relax(incompatibility)
    result = super
    if (log = ENV["RELAX_LOG"])
      File.open(log, "a") { |f| f.puts JSON.generate({ "unlock" => result[0], "pre" => result[1] }) }
    end
    result
  end
end

# `Bundler::Resolver` is loaded after this file, so it is prepended to once
# its class body ends.
TracePoint.new(:end) do |tp|
  if tp.self.name == "Bundler::Resolver" && !tp.self.ancestors.include?(RelaxHook)
    tp.self.prepend(RelaxHook)
  end
end.enable

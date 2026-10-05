# The ruby/spec report, made from the records `SpinelFormatter` keeps (#145).
#
# A record is `[kind, file, description, detail]`: `passed`, `failed` (detail:
# the unmet expectation), `blocked` (detail: why), `skipped` (a `fails` tag),
# `file` (one per spec file loaded), `unloaded` (a file or group that stopped
# while loading), `fixture` (a fixture that stopped part way) and `tag` (a
# `spec/tags/` line that cannot be used; detail: why).
#
# This is one process's report. `scripts/spec.sh` runs many and merges their
# records with `scripts/spec-report.sh`, which prints the same report — in awk,
# because the merge is tens of thousands of records and Spinel is not yet fast
# enough to make that quick. The two are kept in step by hand; the format is
# small.
module SpinelReport
  MILESTONES = "https://github.com/ar4mirez/spinel/milestones"

  # Kinds that are examples. `skipped` is one: a tagged example is still in
  # the corpus, and the examples count is the corpus's size.
  EXAMPLES = %w[passed failed blocked skipped].freeze

  class << self
    def report(records, elapsed = nil)
      kinds = Hash.new { |hash, kind| hash[kind] = [] }
      records.each { |record| kinds[record[0]] << record }
      out = +""
      unless kinds["failed"].empty?
        out << "failed (#{kinds["failed"].size}):\n"
        kinds["failed"].each { |record| out << "  #{record[1]} #{record[2]}: #{record[3]}\n" }
        out << "\n"
      end
      out << "#{kinds["file"].size} files · #{EXAMPLES.sum { |kind| kinds[kind].size }} examples · " \
             "#{kinds["passed"].size} passed · #{kinds["failed"].size} failed · " \
             "#{kinds["blocked"].size} blocked · #{kinds["skipped"].size} skipped"
      out << " · #{elapsed}" if elapsed
      out << "\n"
      section(out, "fixtures that stopped part way", kinds["fixture"])
      section(out, "stopped while loading", kinds["unloaded"])
      section(out, "tag problems, see spec/tags/README.md", kinds["tag"])
      unless kinds["blocked"].empty?
        counts = Hash.new(0)
        kinds["blocked"].each { |record| counts[record[3]] += 1 }
        ranked = counts.sort_by { |reason, count| [-count, reason] }
        out << "\nblocked by, most examples first (#{MILESTONES}):\n"
        ranked.first(20).each { |reason, count| out << "#{count.to_s.rjust(7)}  #{reason}\n" }
        out << "  ... and #{ranked.size - 20} more reasons\n" if ranked.size > 20
      end
      out
    end

    def section(out, heading, records)
      return if records.empty?
      out << "\n#{heading} (#{records.size}):\n"
      records.each { |record| out << "  #{record[1]}: #{record[3]}\n" }
    end

    # Everything wrong with the `spec/tags/` files of the spec files in these
    # records, as `tag` records. mspec drops a tag it cannot parse without a
    # word, and skips nothing for one naming an example that is gone; either
    # is a skip that silently stopped happening, so each fails the run.
    # Spinel's rules on top of mspec's are in `spec/tags/README.md`: a reason
    # is required, and `fails` is the only tag.
    def tag_problems(records, root)
      examples = Hash.new { |hash, file| hash[file] = {} }
      records.each { |record| examples[record[1]][record[2]] = true if EXAMPLES.include?(record[0]) }
      # A file or group that stopped while loading never defined the
      # examples after the stop, so whether a tag's example exists there
      # cannot be told; the format is still checked.
      stopped = {}
      records.each { |record| stopped[record[1]] = true if record[0] == "unloaded" }
      problems = []
      records.each do |record|
        next unless record[0] == "file"
        tags = "spec/tags/" + record[1].sub(%r{\Aspec/ruby/}, "").sub(/_spec\.rb\z/, "_tags.txt")
        next unless File.exist?(File.join(root, tags))
        File.read(File.join(root, tags)).split("\n").each_with_index do |line, index|
          next if line.strip.empty?
          why = tag_problem(line, stopped[record[1]] ? nil : examples[record[1]])
          problems << ["tag", "#{tags}:#{index + 1}", "", why] if why
        end
      end
      problems
    end

    def tag_problem(line, examples)
      match = /\A([^()#:]+)(?:\(([^)]*)\))?:(.*)\z/.match(line)
      if match.nil?
        "not `fails(reason):description`, or a reason with a parenthesis in it"
      elsif match[1] != "fails"
        "`#{match[1]}` is not a tag Spinel honours; only `fails` is"
      elsif match[2].nil? || match[2].strip.empty?
        "a tag needs a reason"
      elsif match[2].include?("(")
        "a reason may not contain a parenthesis"
      elsif examples && !examples.key?(match[3])
        "no example named `#{match[3]}`"
      end
    end
  end
end

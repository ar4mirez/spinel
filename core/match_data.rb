# MatchData.
#
# `[]`, `to_a`, `captures`, `pre_match`, `post_match`, `begin`, `end`, `size`
# and `length` are primitives: they read capture offsets the regexp engine
# recorded. The rest is Ruby.
class MatchData
  # The whole match, which is group 0. Without this, `Kernel#to_s` would answer
  # `#<MatchData>` — a plausible string that is not the matched text.
  def to_s
    self[0]
  end

  def values_at(*indices)
    indices.map { |index| self[index] }
  end

  def to_ary
    to_a
  end

  # Two MatchData are equal when the same pattern matched the same subject in
  # the same places. Ruby compares the regexp, the string, and the offsets;
  # `to_a` is the offsets, read out.
  def ==(other)
    return true if equal?(other)
    return false unless other.is_a?(MatchData)
    regexp == other.regexp && string == other.string && to_a == other.to_a
  end

  def eql?(other)
    self == other
  end
end

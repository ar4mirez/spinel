# MatchData.
#
# `[]`, `to_a`, `captures`, `pre_match`, `post_match`, `begin`, `end`, `size`,
# `length` and `names` are primitives: they read capture offsets and group
# names the regexp engine recorded. The rest is Ruby.
class MatchData
  # The whole match, which is group 0. Without this, `Kernel#to_s` would answer
  # `#<MatchData>` — a plausible string that is not the matched text.
  def to_s
    self[0]
  end

  # `#<MatchData "HX1138" 1:"H" 2:"X">`, or the named form when the pattern
  # declares names — Ruby shows one or the other, never both. Without this,
  # `Object#inspect` would fall through to `to_s` and answer the matched text:
  # a plausible String that is not what `inspect` means.
  def inspect
    text = "#<MatchData " + self[0].inspect
    declared = names
    if declared.empty?
      i = 1
      while i < size
        text = text + " " + i.to_s + ":" + self[i].inspect
        i = i + 1
      end
    else
      declared.each { |name| text = text + " " + name + ":" + self[name].inspect }
    end
    text + ">"
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

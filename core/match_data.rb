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

# What a match knows beyond its text: where each group lay, in bytes and in
# characters, and the groups by name (#33). Over two readings the engine keeps
# — the byte offsets of every group, and the regexp's table of group names.
class MatchData
  alias __at__ []

  # `[]`, `begin` and `end` are primitives for a number or a name, which is
  # what `gsub`, `scan` and `split` ask them in their inner loops. What needs
  # a send to read — a Range, or an object that converts with `to_int` — is
  # handed on to these.
  def __at_other__(key)
    return to_a[key] if key.is_a?(Range)
    self[Integer.__index__(key)]
  end

  def __begin_other__(group) = offset(group)[0]
  def __end_other__(group) = offset(group)[1]

  # Which group a number or a name means. A name that labels several groups
  # means the last of them that took part, and the last of all when none did.
  def __group__(key)
    if key.is_a?(Symbol) || key.is_a?(String)
      name = key.to_s
      table = regexp.__name_table__
      offsets = __offsets__
      found = nil
      index = 0
      while index < table.size
        if table[index] == name
          group = table[index + 1]
          found = group if found.nil? || !offsets[group * 2].nil? || offsets[found * 2].nil?
        end
        index = index + 2
      end
      raise IndexError, "undefined group name reference: " + name if found.nil?
      return found
    end
    group = Integer.__index__(key)
    raise IndexError, "index " + group.to_s + " out of matches" if group < 0 || group >= size
    group
  end

  def byteoffset(group)
    group = __group__(group)
    offsets = __offsets__
    [offsets[group * 2], offsets[group * 2 + 1]]
  end

  def bytebegin(group) = byteoffset(group)[0]
  def byteend(group) = byteoffset(group)[1]

  # The same in characters: how many lie before each byte offset.
  def offset(group)
    first, last = byteoffset(group)
    return [nil, nil] if first.nil?
    subject = string
    [subject.byteslice(0, first).size, subject.byteslice(0, last).size]
  end

  def match(group)
    __at__(__group__(group))
  end

  def match_length(group)
    text = match(group)
    text.nil? ? nil : text.size
  end

  # Numbers and Ranges pick as they do from an Array; a name picks its group.
  def values_at(*indices)
    all = to_a
    out = []
    indices.each do |index|
      if index.is_a?(Symbol) || index.is_a?(String)
        out.push(all[__group__(index)])
      else
        out.concat(all.values_at(index))
      end
    end
    out
  end

  # Each name and what it captured. Keys are Strings unless asked otherwise.
  def named_captures(symbolize_names: false)
    out = {}
    names.each do |name|
      out[symbolize_names ? name.to_sym : name] = __at__(__group__(name))
    end
    out
  end

  alias deconstruct captures

  # For `in {year:, month:}`: the named groups asked for, stopping at the
  # first name the pattern does not have. nil asks for all of them.
  def deconstruct_keys(keys)
    return named_captures(symbolize_names: true) if keys.nil?
    unless keys.is_a?(Array)
      raise TypeError, "wrong argument type " + keys.class.to_s + " (expected Array)"
    end
    declared = names
    out = {}
    return out if keys.size > declared.size
    keys.each do |key|
      unless key.is_a?(Symbol)
        raise TypeError, "wrong argument type " + key.class.to_s + " (expected Symbol)"
      end
      return out unless declared.include?(key.to_s)
      out[key] = __at__(__group__(key))
    end
    out
  end

  def hash
    [MatchData, regexp, string, __offsets__].hash
  end
end

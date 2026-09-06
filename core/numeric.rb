# Numeric — the coercion protocol, and the operators that reach it.
#
# Ruby's numeric operators do not give up on an operand they do not recognise.
# They ask it to `coerce` itself, expect a two-element array back, and retry the
# operator on that pair. `1 + obj` is not "Integer#+ with a bad argument"; it is
# "ask obj for a pair of numbers, then add those".
#
# Every method here is reached only after the VM's fast path has declined.
# `Insn::BinOp` answers fixnum and flonum pairs without dispatching at all, and
# falls back to an ordinary send when it cannot — so by the time one of these
# runs, the pair is either not both numbers, or is a pair this VM has no
# representation for. Those two cases are told apart below, because they need
# opposite answers: one coerces, the other must refuse.
#
# The protocol has one asymmetry, and it is not a wart to be smoothed over:
# `<=>` answers `nil` exactly where the arithmetic operators raise. Measured
# into `crates/spinel-vm/tests/coerce.txt`, along with what happens when
# `coerce` raises, answers the wrong shape, or answers a pair of the wrong type.
class Numeric
  # `[Float(other), Float(self)]` — except between two of the same class, where
  # the pair is handed back untouched. That exception is what keeps
  # `1.coerce(2)` a pair of Integers rather than a pair of Floats.
  def coerce(other)
    return [other, self] if other.class.equal?(self.class)
    [__to_float__(other), __to_float__(self)]
  end

  # The ten operators, written out rather than routed through one `send`.
  #
  # The retry has to be the operator *syntax*: `pair[0] + pair[1]` compiles to
  # `Insn::BinOp`, whose fast path adds two numbers without dispatching, while
  # `pair[0].send(:+, pair[1])` would come straight back here and recurse. It is
  # also what makes the retry land on the right receiver — `"a" + "b"` when
  # `coerce` answered a pair of Strings.
  def +(other)
    pair = __coerce_bin__(other, :+)
    pair[0] + pair[1]
  end

  def -(other)
    pair = __coerce_bin__(other, :-)
    pair[0] - pair[1]
  end

  def *(other)
    pair = __coerce_bin__(other, :*)
    pair[0] * pair[1]
  end

  def /(other)
    pair = __coerce_bin__(other, :/)
    pair[0] / pair[1]
  end

  def %(other)
    pair = __coerce_bin__(other, :%)
    pair[0] % pair[1]
  end

  # ponytail: `**` is not an `Insn::BinOp` and no class implements it, so a
  # coerced pair of numbers still refuses here. That is the missing operator,
  # not the missing protocol; this line starts working the day `**` does.
  def **(other)
    pair = __coerce_bin__(other, :**)
    pair[0] ** pair[1]
  end

  def <(other)
    pair = __coerce_relop__(other, :<)
    __relop_answer__(pair.nil? ? nil : pair[0] < pair[1], other)
  end

  def <=(other)
    pair = __coerce_relop__(other, :<=)
    __relop_answer__(pair.nil? ? nil : pair[0] <= pair[1], other)
  end

  def >(other)
    pair = __coerce_relop__(other, :>)
    __relop_answer__(pair.nil? ? nil : pair[0] > pair[1], other)
  end

  def >=(other)
    pair = __coerce_relop__(other, :>=)
    __relop_answer__(pair.nil? ? nil : pair[0] >= pair[1], other)
  end

  # The one operator that swallows the failure. An operand with no `coerce`, or
  # a `coerce` answering nil, is "no opinion" rather than an error — which is
  # what lets `1.0 <=> "1"` answer nil while `1.0 + "1"` raises. That asymmetry
  # is the whole of the difference between `rb_num_coerce_cmp` and
  # `rb_num_coerce_bin`, and it is measured, not inferred.
  def <=>(other)
    if other.is_a?(Numeric)
      return -1 if self < other
      return 1 if self > other
      return 0
    end
    pair = __coerce_pair__(other, false)
    return nil if pair.nil?
    pair[0] <=> pair[1]
  end

  # `other.coerce(self)`, checked.
  #
  # `strict` is CRuby's `err` flag, and it decides only what an *absent* or nil
  # answer means: an error for the arithmetic operators, "no opinion" for `<=>`
  # and the relational ones. A `coerce` that answers the wrong *shape* is a
  # TypeError either way — that is a broken object, not a refusal to compare.
  def __coerce_pair__(other, strict)
    unless other.respond_to?(:coerce)
      return nil unless strict
      raise TypeError, other.class.to_s + " can't be coerced into " + self.class.to_s
    end
    pair = other.coerce(self)
    return nil if pair.nil? && !strict
    unless pair.is_a?(Array) && pair.size == 2
      raise TypeError, "coerce must return [x, y]"
    end
    pair
  end

  def __coerce_bin__(other, op)
    __refuse_unrepresentable__(other, op)
    __coerce_pair__(other, true)
  end

  # The relational operators retry on the operator itself, not on `<=>`. The two
  # differ where the coerced pair has one and not the other: `nil < nil` is a
  # NoMethodError, while `(nil <=> nil) < 0` would quietly answer false.
  def __coerce_relop__(other, op)
    __refuse_unrepresentable__(other, op)
    __coerce_pair__(other, false)
  end

  # A relational operator that cannot answer raises rather than answering false,
  # and so does one whose retry answered nil.
  def __relop_answer__(answer, other)
    __cmp_failed__(other) if answer.nil?
    answer
  end

  # Both operands are numbers and the fast path still declined, so the pair has
  # no representation here: an Integer wider than a fixnum, or a result outside
  # flonum range. `coerce` would hand the same pair straight back and this
  # method would call itself forever, so it refuses instead — with the very
  # error the VM raised before this file existed, which is what keeps those
  # examples reported *blocked* rather than silently wrong.
  #
  # ponytail: the ceiling is the missing bignum and boxed-float; when those
  # land this guard goes, because the fast path will stop declining.
  def __refuse_unrepresentable__(other, op)
    if other.is_a?(Numeric)
      raise NoMethodError,
            "undefined method '" + op.to_s + "' for an instance of " + self.class.to_s
    end
  end

  # `Float(value)`, for the types this VM has. An object with no `to_f` cannot
  # be made a Float, and neither can nil, which has one and still refuses.
  #
  # ponytail: CRuby's `Float()` parses a String strictly — `Float(":)")` is an
  # ArgumentError where `":)".to_f` is 0.0 — so `String` will need naming here
  # rather than falling through to `to_f` once `String#to_f` exists.
  def __to_float__(value)
    return value if value.is_a?(Float)
    if value.nil? || !value.respond_to?(:to_f)
      raise TypeError, "can't convert " + __operand_name__(value) + " into Float"
    end
    value.to_f
  end
end

# Encoding (#19).
#
# The list is CRuby's, generated into `crates/spinel-vm/src/encoding_table.rs`
# by `scripts/encoding-table.rb`: every name, alias, constant and flag is a
# measurement. A String carries an index into it. The VM walks characters for
# three encodings — UTF-8, US-ASCII and BINARY — and refuses a character
# operation in any other rather than counting wrongly; the rest of this file is
# names and rules, which every encoding has.
class Encoding
  # The names CRuby resolves from the process at boot rather than from the
  # table. Spinel's process is UTF-8, as CRuby's is under a UTF-8 locale.
  RUNTIME_ALIASES = ["locale", "external", "filesystem"].freeze

  def __index__
    @__index__
  end

  def name
    @__names__[0]
  end

  def to_s
    name
  end

  # The runtime aliases are names of the default external encoding, and of no
  # other. Measured: `Encoding::UTF_8.names` ends with them.
  def names
    list = @__names__.dup
    list.concat(RUNTIME_ALIASES) if equal?(Encoding.default_external)
    list
  end

  def ascii_compatible?
    @__ascii_compatible__
  end

  def dummy?
    @__dummy__
  end

  def inspect
    return "#<Encoding:BINARY (ASCII-8BIT)>" if @__index__ == 0
    "#<Encoding:#{name}#{@__dummy__ ? ' (dummy)' : ''}>"
  end

  # An encoding is a value object: `dup` and `clone` answer itself, as CRuby's
  # undefined allocator makes them.
  def dup
    raise TypeError, "allocator undefined for Encoding"
  end

  def clone(freeze: nil)
    raise TypeError, "allocator undefined for Encoding"
  end

  # Every encoding object, `LIST`, and every constant, built by one
  # primitive: boot runs this once per heap.
  __encoding_install__

  class << self
    undef_method :new

    def allocate
      raise TypeError, "allocator undefined for Encoding"
    end

    def list
      LIST.dup
    end

    # The encoding at a table index: what `__ENCODING__` compiles to.
    def __at__(index)
      LIST[index]
    end

    def name_list
      names = []
      LIST.each { |encoding| names.concat(encoding.__names_without_runtime__) }
      names.concat(RUNTIME_ALIASES)
      names.push("internal")
      names
    end

    def aliases
      table = {}
      LIST.each do |encoding|
        own = encoding.__names_without_runtime__
        own.drop(1).each { |name| table[name] = own[0] }
      end
      RUNTIME_ALIASES.each { |name| table[name] = default_external.name }
      internal = default_internal
      table["internal"] = internal.name unless internal.nil?
      table
    end

    # By name, ignoring case; an Encoding is itself. `internal` is the default
    # internal encoding, which may be nil.
    def find(name)
      return name if name.is_a?(Encoding)
      unless name.is_a?(String)
        unless name.respond_to?(:to_str)
          raise TypeError, "no implicit conversion of #{name.nil? ? 'nil' : name.class} into String"
        end
        name = name.to_str
      end
      key = name.__ascii_upcase__
      return default_external if key == "LOCALE" || key == "EXTERNAL" || key == "FILESYSTEM"
      return default_internal if key == "INTERNAL"
      found = __by_name__[key]
      return found unless found.nil?
      raise ArgumentError, "unknown encoding name - #{name}"
    end

    # Every table name, ASCII-upcased, to its encoding; built on first use.
    def __by_name__
      return @__by_name__ unless @__by_name__.nil?
      table = {}
      LIST.each do |encoding|
        encoding.__names_without_runtime__.each { |candidate| table[candidate.__ascii_upcase__] = encoding }
      end
      @__by_name__ = table
    end

    def default_external
      @__default_external__ || Encoding::UTF_8
    end

    def default_external=(encoding)
      raise ArgumentError, "default external can not be nil" if encoding.nil?
      @__default_external__ = find(encoding)
    end

    def default_internal
      @__default_internal__
    end

    def default_internal=(encoding)
      @__default_internal__ = encoding.nil? ? nil : find(encoding)
    end

    def locale_charmap
      "UTF-8"
    end

    # CRuby's `enc_compatible_latter`, ported line for line — including that
    # after swapping a non-string to the right it still answers by the
    # original sides' encodings.
    def compatible?(left, right)
      enc1 = __encoding_of__(left)
      enc2 = __encoding_of__(right)
      return nil if enc1.nil? || enc2.nil?
      return enc1 if enc1.equal?(enc2)
      isstr1 = left.is_a?(String)
      isstr2 = right.is_a?(String)
      return enc1 if isstr2 && right.empty?
      if isstr1 && isstr2 && left.empty?
        return enc1.ascii_compatible? && right.ascii_only? ? enc1 : enc2
      end
      return nil unless enc1.ascii_compatible? && enc2.ascii_compatible?
      return enc1 if !isstr2 && enc2.equal?(Encoding::US_ASCII)
      return enc2 if !isstr1 && enc1.equal?(Encoding::US_ASCII)
      unless isstr1
        left, right = right, left
        isstr1, isstr2 = isstr2, isstr1
      end
      if isstr1
        cr1 = left.ascii_only?
        if isstr2
          cr2 = right.ascii_only?
          if cr1 != cr2
            return enc2 if cr1
            return enc1 if cr2
          end
          return enc1 if cr2
        end
        return enc2 if cr1
      end
      nil
    end

    def __encoding_of__(object)
      case object
      when Encoding then object
      when String then object.encoding
      when Symbol then object.to_s.encoding
      when Regexp then object.source.ascii_only? ? Encoding::US_ASCII : Encoding::UTF_8
      end
    end
  end

  def __names_without_runtime__
    @__names__
  end

  class CompatibilityError < EncodingError; end
  class UndefinedConversionError < EncodingError; end
  class InvalidByteSequenceError < EncodingError; end
  class ConverterNotFoundError < EncodingError; end
end

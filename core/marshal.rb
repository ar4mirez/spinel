# Marshal — objects to bytes and back, in CRuby's wire format 4.8 (#37).
#
# The format is the contract: what `Marshal.dump` writes here CRuby reads, and
# the other way round. It is written in Ruby, over the reflection the rest of
# the core library already uses; nothing here needed a primitive of its own.
#
# The stream is a tree of tagged values with two tables beside it. Symbols are
# numbered as they are first written and later named by number (`;`), and so
# are objects (`@`) — which is what keeps a shared or self-containing object
# one object after the round trip. The order things enter those tables is part
# of the format, and is why the writer below registers some objects before
# their contents and others after: each case follows `marshal.c`.
module Marshal
  MAJOR_VERSION = 4
  MINOR_VERSION = 8

  # Reflection that must not go through the object: it may be a BasicObject,
  # or define `class`, `freeze` or `respond_to?` itself.
  KERNEL_CLASS = ::Kernel.instance_method(:class)
  KERNEL_RESPOND = ::Kernel.instance_method(:respond_to?)
  KERNEL_IVARS = ::Kernel.instance_method(:instance_variables)
  KERNEL_IVAR_GET = ::Kernel.instance_method(:__ivar_get__)
  KERNEL_IVAR_SET = ::Kernel.instance_method(:__ivar_set__)
  KERNEL_FREEZE = ::Kernel.instance_method(:freeze)
  KERNEL_EXTEND = ::Kernel.instance_method(:extend)
  MODULE_NAME = ::Module.instance_method(:name)

  # Where the pairs go that the format calls instance variables and Ruby does
  # not: `offset` and `zone` on a dumped Time, with no `@`.
  EXTRAS = :@__marshal__

  def self.dump(object, *rest)
    port = nil
    limit = -1
    if rest.size > 2
      raise ArgumentError, "wrong number of arguments (given " + (rest.size + 1).to_s + ", expected 1..3)"
    elsif rest.size == 2
      limit = Integer.__index__(rest[1]) unless rest[1].nil?
      raise TypeError, "instance of IO needed" if rest[0].nil?
      port = rest[0]
    elsif rest.size == 1
      if rest[0].is_a?(Integer)
        limit = rest[0]
      elsif rest[0].nil?
        raise TypeError, "instance of IO needed"
      else
        port = rest[0]
      end
    end
    unless port.nil?
      raise TypeError, "instance of IO needed" unless port.respond_to?(:write)
      port.binmode if port.respond_to?(:binmode)
    end
    writer = Writer.new
    writer.object(object, limit)
    return writer.out if port.nil?
    port.write(writer.out)
    port
  end

  def self.load(source, proc = nil, freeze: false)
    if source.respond_to?(:to_str)
      source = source.to_str
    elsif source.respond_to?(:getbyte) && source.respond_to?(:read)
      source.binmode if source.respond_to?(:binmode)
    else
      raise TypeError, "instance of IO needed"
    end
    reader = Reader.new(source, proc, freeze)
    major = reader.byte
    minor = reader.byte
    if major != MAJOR_VERSION || minor > MINOR_VERSION
      raise TypeError, "incompatible marshal file format (can't be read)\n\tformat version " +
                       MAJOR_VERSION.to_s + "." + MINOR_VERSION.to_s + " required; " +
                       major.to_s + "." + minor.to_s + " given"
    end
    reader.object
  end

  class << self
    alias restore load
  end

  # Whether `object` answers `name`, private methods included. An object that
  # defines `respond_to?` is asked; one that cannot be asked — a BasicObject —
  # is looked at.
  def self.__responds__(object, name)
    if KERNEL_RESPOND.bind(object).call(:respond_to?, true)
      object.respond_to?(name, true)
    else
      KERNEL_RESPOND.bind(object).call(name, true)
    end
  end

  # The class or module a dumped path names, looked up one constant at a
  # time and never through an ancestor.
  def self.__constant__(path)
    scope = ::Object
    walked = nil
    path.split("::").each do |name|
      walked = walked.nil? ? name : walked + "::" + name
      found = begin
        scope.const_defined?(name, false)
      rescue ::NameError
        false
      end
      raise ::ArgumentError, "undefined class/module " + walked unless found
      scope = scope.const_get(name, false)
      unless ::Module === scope
        raise ::TypeError, walked + " does not refer to class/module"
      end
    end
    raise ::ArgumentError, "undefined class/module " + path if walked.nil?
    scope
  end

  # Classes whose instances are not made of instance variables, so cannot be
  # written as an object nor filled in as one.
  def self.__opaque__
    @__opaque__ ||= %w[Proc Method UnboundMethod Binding Thread Enumerator Fiber Dir
                       Encoding Random Mutex Thread::Mutex Thread::Queue IO MatchData].map do |path|
      begin
        __constant__(path)
      rescue ::ArgumentError, ::TypeError
        nil
      end
    end.compact
  end

  # A library class whose instances CRuby makes in C adds itself here when it
  # loads, after the table above may already be made: `StringIO`.
  def self.__opaque_class__(klass)
    __opaque__ << klass
  end

  def self.__valued__
    @__valued__ ||= [::String, ::Array, ::Hash, ::Regexp, ::Integer, ::Float, ::Symbol, ::Module,
                     ::Struct, ::Data, ::Time, ::NilClass, ::TrueClass, ::FalseClass]
  end

  class Writer
    def initialize
      @out = "".b
      @symbols = {}
      @objects = {}.compare_by_identity
      @count = 0
      @encodings = {}
      @out << MAJOR_VERSION
      @out << MINOR_VERSION
    end

    def out
      @out
    end

    def byte(value)
      @out << (value & 0xff)
    end

    def tag(letter)
      @out << letter
    end

    # An Integer in one to five bytes: small ones in one, the rest as a count
    # of bytes and then those bytes, least significant first.
    def long(value)
      if value == 0
        byte(0)
      elsif value > 0 && value < 123
        byte(value + 5)
      elsif value < 0 && value > -124
        byte(value - 5)
      else
        bytes = []
        size = 0
        1.upto(8) do |index|
          bytes.push(value & 0xff)
          value = value >> 8
          if value == 0
            size = index
            break
          end
          if value == -1
            size = -index
            break
          end
        end
        byte(size)
        bytes.each { |part| byte(part) }
      end
    end

    def bytes(text)
      long(text.bytesize)
      @out << text.b
    end

    def symbol(name)
      index = @symbols[name]
      unless index.nil?
        tag(";")
        long(index)
        return
      end
      text = name.to_s
      encoding = text.ascii_only? ? nil : encoding_of(text)
      tag("I") unless encoding.nil?
      tag(":")
      bytes(text)
      @symbols[name] = @symbols.size
      unless encoding.nil?
        long(1)
        encoding_pair(encoding, 1)
      end
    end

    # A class path is written as a Symbol, so a second object of the class
    # names it by number.
    def unique(path)
      symbol(path.to_sym)
    end

    # nil for binary, false for US-ASCII, true for UTF-8, and the name — one
    # String per dump, so a second use is a link — for anything else.
    def encoding_of(text)
      encoding = text.encoding
      return nil if encoding == ::Encoding::BINARY
      return false if encoding == ::Encoding::US_ASCII
      return true if encoding == ::Encoding::UTF_8
      @encodings[encoding.name] ||= encoding.name.b
    end

    def encoding_pair(encoding, limit)
      if encoding == true || encoding == false
        symbol(:E)
      else
        symbol(:encoding)
      end
      object(encoding, limit)
    end

    def remember(value)
      @objects[value] = @count
      @count += 1
    end

    def object(value, limit)
      raise ::ArgumentError, "exceed depth limit" if limit == 0
      if nil.equal?(value)
        tag("0")
      elsif true.equal?(value)
        tag("T")
      elsif false.equal?(value)
        tag("F")
      elsif ::Integer === value
        integer(value)
      elsif ::Symbol === value
        symbol(value)
      else
        index = @objects[value]
        unless index.nil?
          tag("@")
          long(index)
          return
        end
        limit -= 1 if limit > 0
        if ::Float === value
          remember(value)
          tag("f")
          bytes(float_text(value))
        elsif Marshal.__responds__(value, :marshal_dump)
          remember(value)
          state = value.__send__(:marshal_dump)
          class_of("U", value, false)
          object(state, limit)
        elsif Marshal.__responds__(value, :_dump)
          user_defined(value, limit)
        else
          remember(value)
          built_in(value, limit)
        end
      end
    end

    # Thirty-one bits fit the `i` form. Anything wider is written as a bignum
    # and takes a place in the object table, though only one too wide for a
    # machine word is ever found there again.
    def integer(value)
      if value >= -1_073_741_824 && value < 1_073_741_824
        tag("i")
        long(value)
        return
      end
      if value >= -4_611_686_018_427_387_904 && value < 4_611_686_018_427_387_904
        @count += 1
      else
        index = @objects[value]
        unless index.nil?
          tag("@")
          long(index)
          return
        end
        remember(value)
      end
      tag("l")
      tag(value < 0 ? "-" : "+")
      magnitude = value.abs
      parts = []
      while magnitude > 0
        parts.push(magnitude & 0xff)
        magnitude = magnitude >> 8
      end
      parts.push(0) if parts.size.odd?
      long(parts.size / 2)
      parts.each { |part| byte(part) }
    end

    # The shortest digits that read back as the same Float, laid out the way
    # `marshal.c` lays them out: no `.0`, and an exponent only when the point
    # falls outside the digits.
    def float_text(value)
      return "nan" if value.nan?
      return value < 0 ? "-inf" : "inf" if value.infinite?
      return (1.0 / value) < 0 ? "-0" : "0" if value == 0.0
      text = value.abs.to_s
      mantissa, exponent = text.split("e")
      exponent = exponent.nil? ? 0 : exponent.to_i
      whole, fraction = mantissa.split(".")
      fraction = "" if fraction.nil?
      digits = whole + fraction
      point = whole.size + exponent
      stripped = digits.sub(/\A0+/, "")
      point -= digits.size - stripped.size
      digits = stripped.sub(/0+\z/, "")
      out = value < 0 ? "-" : ""
      if point < -3 || point > digits.size
        out = out + digits[0]
        out = out + "." + digits[1..] if digits.size > 1
        out + "e" + (point - 1).to_s
      elsif point > 0
        out = out + digits[0, point]
        out = out + "." + digits[point..] if digits.size > point
        out
      else
        out + "0." + ("0" * -point) + digits
      end
    end

    # `_dump` answers the bytes and the class supplies `_load`. The object
    # enters the table last, after any instance variables of that String.
    def user_defined(value, limit)
      text = value.__send__(:_dump, limit)
      raise ::TypeError, "_dump() must return string" unless ::String === text
      pairs = ivar_pairs(text)
      encoding = encoding_of(text)
      if pairs.empty? && encoding.nil? && generic?(value)
        pairs = ivar_pairs(value)
        encoding = ::String === value ? encoding_of(value) : nil
      end
      wrapped = !pairs.empty? || !encoding.nil?
      tag("I") if wrapped
      class_of("u", value, false)
      bytes(text)
      ivars(pairs, encoding, limit) if wrapped
      remember(value)
    end

    # Whether instance variables on `value` are written around it rather
    # than inside it.
    def generic?(value)
      ::String === value || ::Array === value || ::Hash === value || ::Regexp === value ||
        ::Struct === value || ::Data === value
    end

    def ivar_pairs(value)
      pairs = KERNEL_IVARS.bind(value).call.map do |name|
        [name, KERNEL_IVAR_GET.bind(value).call(name)]
      end
      extras = KERNEL_IVAR_GET.bind(value).call(EXTRAS)
      extras.each { |pair| pairs.push(pair) } unless extras.nil?
      pairs
    end

    def ivars(pairs, encoding, limit)
      long(pairs.size + (encoding.nil? ? 0 : 1))
      encoding_pair(encoding, limit) unless encoding.nil?
      pairs.each do |name, held|
        symbol(name)
        object(held, limit)
      end
    end

    def built_in(value, limit)
      if ::Module === value
        return module_path(value)
      end
      klass = KERNEL_CLASS.bind(value).call
      if ::String === value
        wrap(value, encoding_of(value), limit) do
          user_class(value, ::String)
          tag('"')
          bytes(value)
        end
      elsif ::Regexp === value
        encoding = regexp_encoding(value)
        wrap(value, encoding, limit) do
          user_class(value, ::Regexp)
          tag("/")
          bytes(value.source)
          byte(value.options)
        end
      elsif ::Array === value
        wrap(value, nil, limit) do
          user_class(value, ::Array)
          tag("[")
          long(value.size)
          index = 0
          while index < value.size
            object(value[index], limit)
            index += 1
          end
        end
      elsif ::Hash === value
        wrap(value, nil, limit) { hash_body(value, limit) }
      elsif ::Struct === value
        wrap(value, nil, limit) do
          class_of("S", value, true)
          members = klass.__members__
          values = KERNEL_IVAR_GET.bind(value).call(:@__values__) || []
          long(members.size)
          members.each_with_index do |member, index|
            symbol(member)
            object(values[index], limit)
          end
        end
      elsif ::Data === value
        wrap(value, nil, limit) do
          class_of("S", value, true)
          members = klass.__members__
          values = KERNEL_IVAR_GET.bind(value).call(:@__values__) || {}
          long(members.size)
          members.each do |member|
            symbol(member)
            object(values[member], limit)
          end
        end
      elsif ::Range === value
        class_of("o", value, true)
        long(3)
        symbol(:excl)
        object(value.exclude_end?, limit)
        symbol(:begin)
        object(value.begin, limit)
        symbol(:end)
        object(value.end, limit)
      elsif ::Exception === value
        exception(value, limit)
      elsif Marshal.__opaque__.any? { |opaque| opaque === value }
        opaque(value, klass, limit)
      else
        class_of("o", value, true)
        ivars(ivar_pairs(value), nil, limit)
      end
    end

    # A value with a representation of its own keeps its instance variables
    # outside it: `I`, the value, then the pairs.
    def wrap(value, encoding, limit)
      pairs = ivar_pairs(value)
      wrapped = !pairs.empty? || !encoding.nil?
      tag("I") if wrapped
      yield
      ivars(pairs, encoding, limit) if wrapped
    end

    # ponytail: a Regexp here keeps one encoding and answers UTF-8 for a
    # binary source, which CRuby writes with none. Follow `Regexp#encoding`
    # when it tells the two apart (#33).
    def regexp_encoding(value)
      encoding = value.encoding
      return nil if encoding == ::Encoding::BINARY
      return false if encoding == ::Encoding::US_ASCII
      return true if encoding == ::Encoding::UTF_8
      @encodings[encoding.name] ||= encoding.name.b
    end

    def hash_body(value, limit)
      user_class(value, ::Hash)
      unless value.default_proc.nil?
        raise ::TypeError, "can't dump hash with default proc"
      end
      if value.compare_by_identity?
        tag("C")
        unique("Hash")
      end
      default = value.default
      tag(default.nil? ? "{" : "}")
      long(value.size)
      value.each_pair do |key, held|
        object(key, limit)
        object(held, limit)
      end
      object(default, limit) unless default.nil?
    end

    # `mesg` and `bt` are what CRuby calls the message and the backtrace,
    # and they are written first, as it writes them.
    #
    # ponytail: the name a NameError carries and the key a KeyError carries
    # are ordinary instance variables here, so they travel as `@name` rather
    # than CRuby's `name`. Rename them when an exception's state stops being
    # instance variables.
    def exception(value, limit)
      class_of("o", value, true)
      pairs = ivar_pairs(value)
      cause = KERNEL_IVAR_GET.bind(value).call(:@__cause__)
      long(pairs.size + (cause.nil? ? 2 : 3))
      symbol(:mesg)
      object(KERNEL_IVAR_GET.bind(value).call(:@__message__), limit)
      symbol(:bt)
      object(KERNEL_IVAR_GET.bind(value).call(:@__backtrace__), limit)
      unless cause.nil?
        symbol(:cause)
        object(cause, limit)
      end
      pairs.each do |name, held|
        symbol(name)
        object(held, limit)
      end
    end

    def opaque(value, klass, limit)
      if ::IO === value || ::MatchData === value
        raise ::TypeError, "can't dump " + MODULE_NAME.bind(klass).call.to_s
      end
      unless Marshal.__responds__(value, :_dump_data)
        raise ::TypeError, "no _dump_data is defined for class " + MODULE_NAME.bind(klass).call.to_s
      end
      state = value.__send__(:_dump_data)
      class_of("d", value, true)
      object(state, limit)
    end

    def module_path(value)
      if ::Class === value
        raise ::TypeError, "singleton class can't be dumped" if value.singleton_class?
        path = path_of(value, "class")
        letter = "c"
      else
        path = path_of(value, "module")
        letter = "m"
      end
      plain = path.ascii_only?
      tag("I") unless plain
      tag(letter)
      bytes(path)
      unless plain
        long(1)
        encoding_pair(true, 1)
      end
    end

    # A class's own name, whatever a `name` method of its says, and only if
    # that name still leads to it.
    def path_of(klass, kind)
      path = MODULE_NAME.bind(klass).call
      if path.nil? || path.start_with?("#<")
        raise ::TypeError, "can't dump anonymous " + kind + " " + klass.inspect
      end
      unless Marshal.__constant__(path).equal?(klass)
        raise ::TypeError, path + " can't be referred to"
      end
      path
    end

    # The modules an object was extended with, outermost first, each as `e`.
    # A singleton class with anything of its own in it cannot be written.
    def extended(value)
      actual = __reflect_class_of__(value)
      return unless __reflect_is_singleton__(actual)
      unless actual.instance_methods(false).empty? && actual.private_instance_methods(false).empty? &&
             actual.instance_variables.empty?
        raise ::TypeError, "singleton can't be dumped"
      end
      klass = KERNEL_CLASS.bind(value).call
      seen = false
      actual.ancestors.each do |ancestor|
        break if ancestor.equal?(klass)
        if ancestor.equal?(actual)
          seen = true
          next
        end
        next unless seen
        tag("e")
        unique(path_of(ancestor, "class"))
      end
    end

    def class_of(letter, value, check)
      klass = KERNEL_CLASS.bind(value).call
      extended(value) if check
      tag(letter)
      unique(path_of(klass, "class"))
    end

    def user_class(value, base)
      extended(value)
      klass = KERNEL_CLASS.bind(value).call
      return if klass.equal?(base)
      tag("C")
      unique(path_of(klass, "class"))
    end
  end

  class Reader
    def initialize(source, proc, freeze)
      if ::String === source
        @data = source.b
        @at = 0
      else
        @io = source
      end
      @proc = proc
      @freeze = freeze
      @symbols = []
      @objects = []
      @partial = {}.compare_by_identity
      @interned = {}
    end

    def byte
      if @io.nil?
        raise ::ArgumentError, "marshal data too short" if @at >= @data.bytesize
        value = @data.getbyte(@at)
        @at += 1
        return value
      end
      value = @io.getbyte
      raise ::EOFError, "end of file reached" if value.nil?
      value
    end

    def long
      head = byte
      head -= 256 if head > 127
      return 0 if head == 0
      if head > 0
        return head - 5 if head > 4
        value = 0
        head.times { |index| value |= byte << (8 * index) }
        value
      else
        return head + 5 if head < -4
        size = -head
        value = -1
        size.times do |index|
          value &= ~(0xff << (8 * index))
          value |= byte << (8 * index)
        end
        value
      end
    end

    def bytes
      take(long)
    end

    def take(size)
      raise ::ArgumentError, "marshal data too short" if size < 0
      return "".b if size == 0
      if @io.nil?
        raise ::ArgumentError, "marshal data too short" if @at + size > @data.bytesize
        text = @data.byteslice(@at, size)
        @at += size
        return text
      end
      text = @io.read(size)
      if text.nil? || text.bytesize != size
        raise ::ArgumentError, "marshal data too short"
      end
      text.b
    end

    def enter(value)
      @objects.push(value)
      @partial[value] = true
      value
    end

    # A place in the object table taken before the object exists, for the
    # values that are made only once their contents are read.
    def reserve
      @objects.push(nil)
      @objects.size - 1
    end

    def enter_at(index, value)
      @objects[index] = value
      @partial[value] = true
      value
    end

    # A finished value: frozen if that was asked for, then shown to the proc,
    # whose answer takes its place. `partial` is a value whose instance
    # variables are still to come; whoever reads those finishes it.
    def leave(value, partial)
      return value if partial
      @partial.delete(value)
      value = frozen(value) if @freeze
      posted(value)
    end

    def frozen(value)
      return value if ::Module === value
      if value.instance_of?(::String) && KERNEL_IVARS.bind(value).call.empty?
        table = (@interned[value.encoding] ||= {})
        found = table[value]
        return found unless found.nil?
        KERNEL_FREEZE.bind(value).call
        table[value] = value
        return value
      end
      KERNEL_FREEZE.bind(value).call
      value
    end

    def posted(value)
      @proc.nil? ? value : @proc.call(value)
    end

    def symbol
      wrapped = false
      loop do
        type = byte
        if type == 73 # I
          wrapped = true
        elsif type == 58 # :
          return real_symbol(wrapped ? [true] : nil)
        elsif type == 59 # ;
          raise ::ArgumentError, "dump format error (symlink with encoding)" if wrapped
          return symbol_link
        else
          raise ::ArgumentError, "dump format error for symbol(0x" + type.to_s(16) + ")"
        end
      end
    end

    def symbol_link
      index = long
      raise ::ArgumentError, "bad symbol" if index < 0 || index >= @symbols.size
      @symbols[index]
    end

    def real_symbol(pending)
      text = bytes
      index = @symbols.size
      @symbols.push(nil)
      text.force_encoding(::Encoding::US_ASCII) if text.ascii_only?
      unless pending.nil?
        long.times do
          name = symbol
          encoding = encoding_from(name, object)
          text.force_encoding(encoding) unless encoding.nil?
        end
        pending[0] = false
      end
      @symbols[index] = text.to_sym
    end

    def unique
      symbol.to_s
    end

    # The encoding a pair names, or nil when it is an ordinary pair.
    def encoding_from(name, value)
      if name == :E
        return ::Encoding::UTF_8 if value == true
        return ::Encoding::US_ASCII if value == false
      elsif name == :encoding && ::String === value
        return ::Encoding.find(value)
      end
      nil
    end

    # The pairs after an `I`: an encoding for what can carry one, and
    # instance variables for the rest.
    def ivars(value)
      long.times do
        name = symbol
        held = object
        encoding = encoding_from(name, held)
        if !encoding.nil?
          unless ::String === value
            raise ::ArgumentError, value.inspect + " is not enc_capable"
          end
          value.force_encoding(encoding)
        else
          set(value, name, held)
        end
      end
    end

    def set(value, name, held)
      if name.to_s.start_with?("@")
        KERNEL_IVAR_SET.bind(value).call(name, held)
      else
        extras = KERNEL_IVAR_GET.bind(value).call(EXTRAS)
        if extras.nil?
          extras = []
          KERNEL_IVAR_SET.bind(value).call(EXTRAS, extras)
        end
        extras.push([name, held])
      end
    end

    def object(partial = false, pending = nil, modules = nil)
      typed(byte, partial, pending, modules, nil)
    end

    def typed(type, partial, pending, modules, klass)
      case type
      when 48 then leave(nil, false)   # 0
      when 84 then leave(true, false)  # T
      when 70 then leave(false, false) # F
      when 105 then leave(long, false) # i
      when 64 # @
        index = long
        if index < 0 || index >= @objects.size
          raise ::ArgumentError, "dump format error (unlinked)"
        end
        value = @objects[index]
        @partial.key?(value) ? value : posted(value)
      when 58 # :
        leave(real_symbol(pending), partial)
      when 59 # ;
        symbol_link
      when 73 # I
        inner = [true]
        value = object(true, inner, modules)
        ivars(value) if inner[0]
        leave(value, partial)
      when 101 # e
        extended(partial, pending)
      when 67 # C
        user_class(partial, pending, modules)
      when 34 # "
        text = bytes
        unless klass.nil?
          bad_user_class unless klass <= ::String
          made = klass.allocate
          ::String.instance_method(:replace).bind(made).call(text)
          text = made
        end
        leave(enter(text), partial)
      when 47 # /
        regexp(partial, pending, klass)
      when 91 # [
        size = long
        array = []
        unless klass.nil?
          bad_user_class unless klass <= ::Array
          array = klass.allocate
        end
        enter(array)
        push = ::Array.instance_method(:push).bind(array)
        size.times { push.call(object) }
        leave(array, partial)
      when 123, 125 # { }
        read_hash(type, partial, klass, false)
      when 102 # f
        leave(enter(float(bytes)), partial)
      when 108 # l
        leave(enter(bignum), partial)
      when 99, 109, 77 # c m M
        constant(type, partial, pending)
      when 83 # S
        struct(partial)
      when 111 # o
        plain(partial)
      when 117 # u
        user_defined(partial, pending)
      when 85 # U
        user_marshal(modules)
      when 100 # d
        data(partial)
      else
        raise ::ArgumentError, "dump format error(0x" + type.to_s(16) + ")"
      end
    end

    def bad_user_class
      raise ::ArgumentError, "dump format error (user class)"
    end

    def class_named(path)
      klass = Marshal.__constant__(path)
      raise ::ArgumentError, path + " does not refer to class" unless ::Class === klass
      klass
    end

    def float(text)
      return ::Float::NAN if text == "nan"
      return ::Float::INFINITY if text == "inf"
      return -::Float::INFINITY if text == "-inf"
      # ponytail: bytes after a NUL are the low mantissa bits a 1.8 dump
      # carried. The digits before them already read back as the nearest
      # Float, so they are skipped rather than applied.
      stop = text.index("\0")
      text = text[0, stop] unless stop.nil?
      text.to_f
    end

    def bignum
      sign = byte
      text = take(long * 2)
      value = 0
      text.bytesize.times { |index| value |= text.getbyte(index) << (8 * index) }
      sign == 45 ? -value : value
    end

    def constant(type, partial, pending)
      path = bytes
      unless pending.nil?
        ivars(path)
        pending[0] = false
      end
      found = Marshal.__constant__(path)
      if type == 99 && !(::Class === found)
        raise ::ArgumentError, path + " does not refer to class"
      end
      if type == 109 && ::Class === found
        raise ::ArgumentError, path + " does not refer to module"
      end
      leave(enter(found), partial)
    end

    # `e`, once per module, then the object they extend. The modules go on
    # innermost first, which leaves the first one written in front.
    def extended(partial, pending)
      modules = []
      type = 101
      while type == 101
        path = unique
        found = Marshal.__constant__(path)
        if ::Class === found
          raise ::ArgumentError, path + " does not refer to module"
        end
        modules.push(found)
        type = byte
      end
      value = typed(type, true, pending, modules, nil)
      modules.reverse_each { |found| KERNEL_EXTEND.bind(value).call(found) }
      # A `U` has already been finished, with its modules on it.
      type == 85 ? value : leave(value, partial)
    end

    # `C` names the subclass the next value is an instance of. A second `C`
    # naming Hash itself is how a Hash that compares by identity is marked.
    def user_class(partial, pending, modules)
      klass = class_named(unique)
      type = byte
      identity = false
      if type == 67
        inner = class_named(unique)
        type = byte
        bad_user_class unless inner.equal?(::Hash) && (type == 123 || type == 125)
        identity = true
      elsif klass.equal?(::Hash) && (type == 123 || type == 125)
        identity = true
        klass = nil
      end
      case type
      when 123, 125 then read_hash(type, partial, klass, identity)
      when 34, 47, 91 then typed(type, partial, pending, modules, klass)
      else bad_user_class
      end
    end

    def read_hash(type, partial, klass, identity)
      size = long
      hash = {}
      unless klass.nil?
        bad_user_class unless klass <= ::Hash
        hash = klass.allocate
      end
      hash.compare_by_identity if identity
      enter(hash)
      store = ::Hash.instance_method(:[]=).bind(hash)
      size.times do
        key = object
        store.call(key, object)
      end
      hash.default = object if type == 125
      leave(hash, partial)
    end

    # The source, the options, and — because a Regexp cannot change encoding
    # once made — the pairs that would follow it, read here first.
    def regexp(partial, pending, klass)
      source = bytes
      options = byte
      index = reserve
      pairs = []
      encoded = false
      unless pending.nil?
        long.times do
          name = symbol
          held = object
          encoding = encoding_from(name, held)
          if encoding.nil?
            pairs.push([name, held])
          else
            source.force_encoding(encoding)
            encoded = true
          end
        end
        pending[0] = false
      end
      source.force_encoding(::Encoding::US_ASCII) if !encoded && source.ascii_only?
      klass = ::Regexp if klass.nil?
      bad_user_class unless klass <= ::Regexp
      value = klass.new(source, options)
      enter_at(index, value)
      pairs.each { |name, held| set(value, name, held) }
      leave(value, partial)
    end

    def struct(partial)
      index = reserve
      path = unique
      klass = class_named(path)
      size = long
      data = klass < ::Data
      unless klass < ::Struct || data
        raise ::TypeError, "class " + path + " not a struct"
      end
      members = klass.__members__
      if members.size != size
        raise ::TypeError, "struct " + path + " not compatible (struct size differs)"
      end
      value = klass.allocate
      enter_at(index, value)
      values = []
      members.each do |member|
        name = symbol
        unless name == member
          raise ::TypeError, "struct " + path + " not compatible (:" + name.to_s + " for :" + member.to_s + ")"
        end
        values.push(object)
      end
      # Straight into the object: a struct read back never runs `initialize`.
      if data
        held = {}
        members.each_with_index { |member, at| held[member] = values[at] }
        KERNEL_IVAR_SET.bind(value).call(:@__values__, held)
        KERNEL_FREEZE.bind(value).call
      else
        KERNEL_IVAR_SET.bind(value).call(:@__values__, values)
      end
      leave(value, partial)
    end

    def plain(partial)
      index = reserve
      path = unique
      klass = class_named(path)
      return range(index, klass, partial) if klass <= ::Range
      return exception(index, klass, partial) if klass <= ::Exception
      if Marshal.__opaque__.any? { |opaque| klass <= opaque } ||
         Marshal.__valued__.any? { |valued| klass <= valued }
        raise ::ArgumentError, "dump format error"
      end
      value = klass.allocate
      enter_at(index, value)
      long.times do
        name = symbol
        set(value, name, object)
      end
      leave(value, partial)
    end

    def range(index, klass, partial)
      value = klass.allocate
      enter_at(index, value)
      parts = {}
      long.times do
        name = symbol
        parts[name] = object
      end
      value.__send__(:initialize, parts[:begin], parts[:end], parts[:excl] ? true : false)
      leave(value, partial)
    end

    def exception(index, klass, partial)
      value = klass.allocate
      enter_at(index, value)
      long.times do
        name = symbol
        held = object
        case name
        when :mesg then KERNEL_IVAR_SET.bind(value).call(:@__message__, held)
        when :bt then KERNEL_IVAR_SET.bind(value).call(:@__backtrace__, held)
        when :cause then KERNEL_IVAR_SET.bind(value).call(:@__cause__, held)
        else
          # ponytail: `bt_locations`, and the `name` and `key` CRuby keeps
          # outside instance variables, are read and dropped.
          KERNEL_IVAR_SET.bind(value).call(name, held) if name.to_s.start_with?("@")
        end
      end
      leave(value, partial)
    end

    def user_defined(partial, pending)
      path = unique
      klass = Marshal.__constant__(path)
      unless klass.respond_to?(:_load, true)
        raise ::TypeError, "class " + path + " needs to have method '_load'"
      end
      text = bytes
      unless pending.nil?
        ivars(text)
        pending[0] = false
      end
      value = klass.__send__(:_load, text)
      @objects.push(value)
      @partial[value] = true
      return value if partial
      KERNEL_FREEZE.bind(value).call if @freeze && !value.nil?
      posted(value)
    end

    def user_marshal(modules)
      path = unique
      klass = class_named(path)
      value = klass.allocate
      unless modules.nil?
        modules.reverse_each { |found| KERNEL_EXTEND.bind(value).call(found) }
        modules.clear
      end
      unless Marshal.__responds__(value, :marshal_load)
        raise ::TypeError, "instance of " + path + " needs to have method 'marshal_load'"
      end
      enter(value)
      value.__send__(:marshal_load, object)
      KERNEL_FREEZE.bind(value).call if @freeze
      posted(value)
    end

    def data(partial)
      path = unique
      klass = class_named(path)
      unless Marshal.__opaque__.any? { |opaque| klass <= opaque }
        raise ::ArgumentError, "dump format error"
      end
      unless klass.method_defined?(:_load_data) || klass.private_method_defined?(:_load_data)
        raise ::TypeError, "class " + path + " needs to have instance method '_load_data'"
      end
      value = klass.allocate
      enter(value)
      value.__send__(:_load_data, object)
      leave(value, partial)
    end
  end
end

# A Time is eight bytes — the date and the hour in one word, the rest and the
# microseconds in the other, both read in UTC — and what does not fit goes
# beside them as pairs: the nanoseconds, the offset, the zone's name.
class Time
  def _dump(limit = -1)
    days, rest = @__seconds__.divmod(86_400)
    year, month, day = Time.__civil__(days)
    if year < 1900 || year > 1900 + 0xffff
      # ponytail: CRuby appends the excess to the eight bytes. Nothing asks
      # for a Time that far out yet.
      raise ArgumentError, "year too big to marshal: " + year.to_s + " UTC"
    end
    whole = @__nanoseconds__.floor
    usec = whole / 1000
    nsec = whole % 1000
    nano = @__nanoseconds__ - whole + nsec
    first = (1 << 31) | ((utc? ? 1 : 0) << 30) | ((year - 1900) << 14) | ((month - 1) << 10) |
            (day << 5) | (rest / 3600)
    second = ((rest % 3600 / 60) << 26) | ((rest % 60) << 20) | usec
    text = "".b
    4.times { |index| text << ((first >> (8 * index)) & 0xff) }
    4.times { |index| text << ((second >> (8 * index)) & 0xff) }
    instance_variables.each { |name| text.instance_variable_set(name, instance_variable_get(name)) }
    extras = []
    unless nano == 0
      if nano.is_a?(Rational)
        extras.push([:nano_num, nano.numerator], [:nano_den, nano.denominator])
      else
        extras.push([:nano_num, nano], [:nano_den, 1])
      end
    end
    # Three decimal digits, packed two to a byte: what Ruby 1.9.1 reads.
    if nsec != 0
      packed = "".b
      packed << (((nsec / 100) << 4) | (nsec / 10 % 10))
      packed << ((nsec % 10) << 4) if nsec % 10 != 0
      extras.push([:submicro, packed])
    end
    unless utc?
      offset = utc_offset
      extras.push([:offset, offset])
    end
    # A zone object travels as its name, and has to have one.
    name = zone
    name = name.name unless name.nil? || name.is_a?(String)
    extras.push([:zone, name.is_a?(String) ? name.dup : nil])
    text.instance_variable_set(Marshal::EXTRAS, extras)
    text
  end
  private :_dump

  class << self
    private

    def _load(text)
      extras = {}
      found = text.instance_variable_get(Marshal::EXTRAS)
      found.each { |name, held| extras[name] = held } unless found.nil?
      bytes = text.b
      first = 0
      second = 0
      4.times do |index|
        first |= bytes.getbyte(index).to_i << (8 * index)
        second |= bytes.getbyte(index + 4).to_i << (8 * index)
      end
      utc = false
      if first & (1 << 31) == 0
        # The form before 1.9: seconds and microseconds.
        seconds = first
        nanoseconds = second * 1000
      else
        utc = (first >> 30) & 1 == 1
        year = ((first >> 14) & 0xffff) + 1900
        month = ((first >> 10) & 0xf) + 1
        day = (first >> 5) & 0x1f
        seconds = __days__(year, month, day) * 86_400 + (first & 0x1f) * 3600 +
                  ((second >> 26) & 0x3f) * 60 + ((second >> 20) & 0x3f)
        nanoseconds = (second & 0xfffff) * 1000
        if extras.key?(:nano_num)
          den = extras[:nano_den] || 1
          nanoseconds += den == 1 ? extras[:nano_num] : Rational(extras[:nano_num], den)
        elsif extras[:submicro].is_a?(String)
          packed = extras[:submicro]
          high = packed.getbyte(0).to_i
          nanoseconds += (high >> 4) * 100 + (high & 0xf) * 10 + (packed.getbyte(1).to_i >> 4)
        end
      end
      time = __at__(seconds, nanoseconds)
      text.instance_variables.each do |name|
        time.instance_variable_set(name, text.instance_variable_get(name))
      end
      if utc
        time.utc
      elsif !extras[:offset].nil?
        time.__send__(:__loaded_zone__, extras[:offset], extras[:zone])
      end
      time
    end
  end

  # A Time read back keeps the offset it was written with and the name its
  # zone had then, wherever it is read.
  def __loaded_zone__(offset, name)
    @__zone__ = nil
    @__dst__ = nil
    @__tz__ = nil
    @__mode__ = :fixed
    @__offset__ = offset
    @__abbr__ = name.is_a?(String) ? name : nil
    # A class that can find a zone by name gets the zone object back.
    if name.is_a?(String) && self.class.respond_to?(:find_timezone)
      @__tz__ = self.class.find_timezone(name)
    end
    self
  end
  private :__loaded_zone__
end

class Rational
  def marshal_load(pair)
    __set__(pair[0], pair[1])
  end
  private :marshal_load
end

class Complex
  def marshal_load(pair)
    __set__(pair[0], pair[1])
  end
  private :marshal_load
end

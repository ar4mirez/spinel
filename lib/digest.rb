# `digest` (#58), at the version CRuby 4.0.6 ships (3.2.1). CRuby's is
# `ext/digest`: `digest.c` for `Digest::Instance`, `Class` and `Base`, a C
# file per algorithm, and a little Ruby — `Digest()`, `Digest::SHA2` — beside
# them. Here all of it is Ruby, over one primitive, `__digest__`, which
# answers the digest of a String's bytes for MD5, SHA1, the three SHA2
# lengths and RMD160 (the RustCrypto implementations).
#
# ponytail: an algorithm keeps the bytes it was given and digests them all at
# `finish`, so its memory is the size of what it was fed; `file` reads the
# whole file. An incremental hasher behind the primitive is the upgrade.

module Digest
  VERSION = "3.2.1"

  def self.hexencode(string)
    string = IO.__str__(string)
    string.unpack1("H*")
  end

  # What every digest is, given `update`, `finish` and `reset`: CRuby's
  # `Digest::Instance`, which an object may include to be a digest.
  module Instance
    def update(_string)
      raise RuntimeError, "#{self.class} does not implement update()"
    end

    alias << update

    def finish
      raise RuntimeError, "#{self.class} does not implement finish()"
    end
    private :finish

    def reset
      raise RuntimeError, "#{self.class} does not implement reset()"
    end

    def block_length
      raise RuntimeError, "#{self.class} does not implement block_length()"
    end

    def new
      clone.reset
    end

    # With a String, the digest of it alone, leaving this one reset; without,
    # this one's digest so far, leaving it as it was.
    def digest(string = nil)
      if string.nil?
        clone.__send__(:finish)
      else
        reset
        update(string)
        value = finish
        reset
        value
      end
    end

    def digest!
      value = finish
      reset
      value
    end

    def hexdigest(string = nil) = Digest.hexencode(digest(string))
    def hexdigest! = Digest.hexencode(digest!)
    def base64digest(string = nil) = [digest(string)].pack("m0")
    def base64digest! = [digest!].pack("m0")
    def to_s = hexdigest

    def inspect = "#<#{self.class.name}: #{hexdigest}>"

    # Another digest is equal when the digests are; a String, when it is
    # this one's digest or, at twice the length, its hex.
    def ==(other)
      if Instance === other
        digest == other.digest
      else
        return false unless other.respond_to?(:to_str)
        string = other.to_str
        string == (string.bytesize == digest_length * 2 ? hexdigest : digest)
      end
    end

    def digest_length = digest.bytesize
    def length = digest_length
    alias size length

    def file(name)
      update(File.open(File.__path__(name), "rb") { |f| f.read })
      self
    end
  end

  # A digest class: the class methods hash a String in one call.
  class Class
    include Instance

    def self.digest(string, *args)
      raise ArgumentError, "no data given" if string.nil?
      new(*args).digest(string)
    end

    def self.hexdigest(string, *args) = Digest.hexencode(digest(string, *args))
    def self.base64digest(string, *args) = [digest(string, *args)].pack("m0")
    def self.file(name, *args) = new(*args).file(name)
  end

  # An algorithm-backed digest: CRuby's `Digest::Base`, here keeping the
  # bytes it was given for `__digest__`.
  class Base < Class
    def initialize
      @buffer = "".b
    end

    def initialize_copy(other)
      @buffer = other.__buffer__.dup
    end

    def __buffer__ = @buffer

    def update(string)
      string = IO.__str__(string)
      @buffer << string.b
      self
    end
    alias << update

    def reset
      @buffer = "".b
      self
    end

    def finish = __digest__(self.class::ALGORITHM, @buffer)
    private :finish

    def digest_length = self.class::DIGEST_LENGTH
    def block_length = self.class::BLOCK_LENGTH
  end

  class MD5 < Base
    ALGORITHM = "MD5"
    DIGEST_LENGTH = 16
    BLOCK_LENGTH = 64
  end

  class SHA1 < Base
    ALGORITHM = "SHA1"
    DIGEST_LENGTH = 20
    BLOCK_LENGTH = 64
  end

  class SHA256 < Base
    ALGORITHM = "SHA256"
    DIGEST_LENGTH = 32
    BLOCK_LENGTH = 64
  end

  class SHA384 < Base
    ALGORITHM = "SHA384"
    DIGEST_LENGTH = 48
    BLOCK_LENGTH = 128
  end

  class SHA512 < Base
    ALGORITHM = "SHA512"
    DIGEST_LENGTH = 64
    BLOCK_LENGTH = 128
  end

  class RMD160 < Base
    ALGORITHM = "RMD160"
    DIGEST_LENGTH = 20
    BLOCK_LENGTH = 64
  end

  # `Digest::SHA2.new(bitlen = 256)`: one of the three by its length, as
  # upstream's `digest/sha2.rb` has it.
  class SHA2 < Class
    def initialize(bitlen = 256)
      case bitlen
      when 256 then @sha2 = SHA256.new
      when 384 then @sha2 = SHA384.new
      when 512 then @sha2 = SHA512.new
      else raise ArgumentError, "unsupported bit length: %s" % bitlen.inspect
      end
      @bitlen = bitlen
    end

    def reset
      @sha2.reset
      self
    end

    def update(string)
      @sha2.update(string)
      self
    end
    alias << update

    def finish = @sha2.digest!
    private :finish

    def block_length = @sha2.block_length
    def digest_length = @sha2.digest_length

    def initialize_copy(other)
      @sha2 = other.instance_eval { @sha2.clone }
    end

    def inspect = "#<%s:%d %s>" % [self.class.name, @bitlen, hexdigest]
  end
end

# `Digest(:MD5)`: the class, loaded from `digest/md5` when it is not there.
def Digest(name)
  const = name.to_sym
  return Digest.const_get(const) if Digest.const_defined?(const)
  lib = File.join("digest", name.to_s.downcase)
  begin
    require lib
  rescue LoadError
    raise LoadError, "library not found for class Digest::#{name} -- #{lib}", caller(1)
  end
  unless Digest.const_defined?(const)
    raise NameError, "uninitialized constant Digest::#{name}", caller(1)
  end
  Digest.const_get(const)
end
private :Digest

# `digest/bubblebabble`: Antti Huima's Bubble Babble encoding, CRuby's
# `bubblebabble.c` in Ruby.
require "digest"

module Digest
  BUBBLEBABBLE_VOWELS = "aeiouy"
  BUBBLEBABBLE_CONSONANTS = "bcdfghklmnprstvzx"
  private_constant :BUBBLEBABBLE_VOWELS, :BUBBLEBABBLE_CONSONANTS

  def self.bubblebabble(string)
    string = IO.__str__(string)
    bytes = string.bytes
    vowels = BUBBLEBABBLE_VOWELS
    consonants = BUBBLEBABBLE_CONSONANTS
    out = +"x"
    seed = 1
    i = 0
    loop do
      if i >= bytes.size
        out << vowels[seed % 6] << consonants[16] << vowels[seed / 6]
        break
      end
      byte1 = bytes[i]
      out << vowels[(((byte1 >> 6) & 3) + seed) % 6] << consonants[(byte1 >> 2) & 15] << vowels[((byte1 & 3) + (seed / 6)) % 6]
      break if i + 1 >= bytes.size
      byte2 = bytes[i + 1]
      out << consonants[(byte2 >> 4) & 15] << "-" << consonants[byte2 & 15]
      seed = (seed * 5 + byte1 * 7 + byte2) % 36
      i += 2
    end
    out << "x"
  end

  module Instance
    def bubblebabble = Digest.bubblebabble(digest)
  end

  class Class
    def self.bubblebabble(*args) = Digest.bubblebabble(digest(*args))
  end
end

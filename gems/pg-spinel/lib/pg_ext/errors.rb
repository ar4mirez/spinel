# frozen_string_literal: true

# pg_errors.c: PG::Error, its direct subclasses, and a class per SQLSTATE
# (`errorcodes.rb`, upstream's `errorcodes.def` as data), registered in
# PG::ERROR_CLASSES under its five-character code and, for a class's first
# code, its two-character class.

require "pg_ext/errorcodes"

module PG
  ERROR_CLASSES = {}

  class Error < StandardError
    attr_reader :connection, :result
    alias_method :error, :message
  end

  class ServerError < Error; end
  class UnableToSend < Error; end
  class ConnectionBad < Error; end
  class InvalidResultStatus < Error; end
  class NoResultError < Error; end
  class InvalidChangeOfResultFields < Error; end

  Spinel::ERROR_CODES.each do |name, base, codes|
    superclass = base ? ERROR_CLASSES[base] : ServerError
    klass = const_defined?(name, false) ? const_get(name, false) : const_set(name, Class.new(superclass))
    codes.each { |code| ERROR_CLASSES[code] = klass }
  end

  module Spinel
    # pg_errors.c's lookup_error_class.
    def self.error_class(sqlstate)
      return UnableToSend unless sqlstate
      ERROR_CLASSES[sqlstate] || ERROR_CLASSES[sqlstate[0, 2]] || ServerError
    end
  end
end

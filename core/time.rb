# The part of `Time` mspec's timer needs (#145): the current time, and the
# difference between two as a Float of seconds. The rest of `Time` —
# calendars, zones, formatting — is #32; until then its methods stay
# undefined, so a spec reaching for one is blocked rather than answered.
class Time
  include Comparable

  def self.now(**zone)
    __needs_time__ unless zone.empty?
    seconds, nanoseconds = __sys_clock__(Process::CLOCK_REALTIME)
    __at__(seconds, nanoseconds)
  end

  def self.new(*)
    __needs_time__
  end

  class << self
    private

    def _load(*)
      __needs_time__
    end
  end

  def self.__at__(seconds, nanoseconds)
    time = allocate
    time.__set__(seconds, nanoseconds)
    time
  end

  def __set__(seconds, nanoseconds)
    @seconds = seconds
    @nanoseconds = nanoseconds
  end

  def to_i = @seconds
  alias tv_sec to_i
  def nsec = @nanoseconds
  alias tv_nsec nsec
  def usec = @nanoseconds / 1000
  alias tv_usec usec

  def to_f
    @seconds + @nanoseconds / 1_000_000_000.0
  end

  def -(other)
    if Time === other
      (@seconds - other.to_i) + (@nanoseconds - other.nsec) / 1_000_000_000.0
    else
      __needs_time__
    end
  end

  # Against what is not a Time, the other side is asked and its answer is
  # turned round: CRuby's `rb_invcmp`. Measured.
  def <=>(other)
    unless Time === other
      order = other <=> self
      return nil if order.nil?
      return order > 0 ? -1 : (order < 0 ? 1 : 0)
    end
    [@seconds, @nanoseconds] <=> [other.to_i, other.nsec]
  end

  def eql?(other)
    Time === other && (self <=> other) == 0
  end

  def hash
    [@seconds, @nanoseconds].hash
  end
end

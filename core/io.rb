# The three standard streams (#145, which needs mspec's output to go through
# `$stdout`). An `IO` here is a descriptor and the writing half; opening
# files and reading are #41's.

class IO
  include Enumerable

  SEEK_SET = 0
  SEEK_CUR = 1
  SEEK_END = 2
  SEEK_DATA = 3
  SEEK_HOLE = 4

  def initialize(fd, mode = nil)
    @fileno = fd
    # Measured: only STDERR starts unbuffered.
    @sync = fd == 2
  end

  def fileno
    @fileno
  end

  alias to_i fileno

  def write(*objects)
    written = 0
    objects.each do |object|
      text = object.to_s
      __write__(text, @fileno)
      written += text.bytesize
    end
    written
  end

  def <<(object)
    write(object)
    self
  end

  def print(*objects)
    objects.each { |object| write(object) }
    nil
  end

  def printf(format, *args)
    write(Kernel.format(format, *args))
    nil
  end

  def putc(char)
    write(Integer === char ? (char & 0xff).chr : char.to_s[0])
    char
  end

  # An Array argument is flattened into its elements, measured: `puts [1, [2]]`
  # is two lines, `puts []` is none, and an Array that contains itself prints
  # `[...]` where it recurs rather than looping.
  def puts(*lines)
    if lines.empty?
      write("\n")
      return nil
    end
    __puts_lines__(lines, [])
    nil
  end

  def __puts_lines__(lines, seen)
    lines.each do |line|
      if line.is_a?(Array)
        if seen.any? { |outer| outer.equal?(line) }
          write("[...]\n")
        else
          __puts_lines__(line, seen + [line])
        end
      else
        text = line.nil? ? "" : line.to_s
        text.end_with?("\n") ? write(text) : write(text, "\n")
      end
    end
  end

  def flush
    self
  end

  def fsync
    0
  end

  # ponytail: recorded, not acted on — every write goes straight out. A
  # buffer is #41's, with the rest of `IO`.
  def sync
    @sync
  end

  def sync=(value)
    @sync = value ? true : false
    value
  end

  def tty?
    __fs_isatty__(@fileno)
  end

  alias isatty tty?

  def closed?
    false
  end

  def inspect
    name = { 0 => "<STDIN>", 1 => "<STDOUT>", 2 => "<STDERR>" }[@fileno] || "fd #{@fileno}"
    "#<#{self.class}:#{name}>"
  end
end

STDIN = IO.new(0)
STDOUT = IO.new(1)
STDERR = IO.new(2)
$stdin = STDIN
$stdout = STDOUT
$stderr = STDERR
alias $> $stdout
# `-W1`, which is the default: `warn` prints, and verbose-only warnings do not.
$VERBOSE = false
# The input record separator: what `gets` and `each_line` split on.
$/ = "\n"

module Kernel
  def puts(*lines)
    $stdout.puts(*lines)
  end

  def print(*parts)
    $stdout.print(*parts)
  end

  def p(*values)
    values.each { |value| $stdout.write(value.inspect, "\n") }
    return nil if values.empty?
    values.size == 1 ? values[0] : values
  end

  def warn(*messages, uplevel: nil, category: nil)
    unless uplevel.nil?
      level = Integer.__index__(uplevel)
      raise ArgumentError, "negative level (#{level})" if level < 0
    end
    unless category.nil? || Symbol === category
      raise TypeError, "no implicit conversion of #{category.class} into Symbol" unless category.respond_to?(:to_sym)
      category = category.to_sym
    end
    return nil if $VERBOSE.nil? || messages.empty?
    $stderr.puts(*messages)
    nil
  end

  module_function :puts, :print, :p, :warn
end

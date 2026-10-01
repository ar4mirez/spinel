# Thread, for the one piece of it Phase 2 needs: `Thread::Backtrace::Location`
# (#29), which is what `caller_locations` and `Exception#backtrace_locations`
# answer.
#
# ponytail: there are no threads. `Thread` exists so the constant the location
# class lives under resolves, and every way of starting one refuses by name
# rather than answering as if a thread ran — `Thread.new { x }.join` returning
# without running `x` would be a wrong answer that passes. The refusal is the
# VM's, not a Ruby exception, so a spec expecting `ThreadError` reads it as
# blocked. #45 is `Thread` on the per-Ractor lock, and replaces these.
class Thread
  def self.new(*)
    __needs_threads__
  end

  def self.start(*)
    __needs_threads__
  end

  def self.fork(*)
    __needs_threads__
  end

  # No Thread can exist yet, but the method does: `Thread#raise` is public.
  def raise(*)
    __needs_threads__
  end

  # Measured: CRuby's `Thread` has no allocator.
  def self.allocate
    raise TypeError, "allocator undefined for Thread"
  end

  module Backtrace
    # One frame of a backtrace. Built by `core/exception.rb` and `Kernel` from
    # the `[path, line, label]` triples the VM records, never by a program:
    # CRuby gives this class no allocator.
    class Location
      def self.__from__(path, lineno, label)
        location = allocate
        location.__send__(:__init_location__, path, lineno, label)
        location
      end

      def __init_location__(path, lineno, label)
        @path = path
        @lineno = lineno
        @label = label
      end

      def path
        @path
      end

      def lineno
        @lineno
      end

      # `Foo#bar`, `Foo.bar`, `block in Foo#bar`, `<main>` — CRuby 3.4's label.
      def label
        @label
      end

      # The method's own name, without the owner or any `block in`: `bar` for
      # all three of `Foo#bar`, `Foo.bar` and `block (2 levels) in Foo#bar`.
      # A label that is not a method — `<main>`, `<class:Foo>` — is its own.
      def base_label
        name = @label
        while name.start_with?("block ")
          at = name.index(" in ")
          break if at.nil?
          name = name[at + 4, name.length - at - 4]
        end
        return name if name.start_with?("<")
        cut = nil
        i = name.length - 1
        while i >= 0
          if name[i] == "#" || name[i] == "."
            cut = i
            break
          end
          i -= 1
        end
        cut.nil? ? name : name[cut + 1, name.length - cut - 1]
      end

      # The file resolved to an absolute path, or nil for one that is not a
      # file — `<internal:array>`, or code from `eval`.
      def absolute_path
        return nil if @path.start_with?("<internal:")
        __absolute_path__(@path)
      end

      def to_s
        "#{@path}:#{@lineno}:in '#{@label}'"
      end

      def inspect
        to_s.inspect
      end
    end
  end
end

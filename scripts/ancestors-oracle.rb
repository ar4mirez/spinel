#!/usr/bin/env ruby
# frozen_string_literal: true

# The oracle behind `crates/spinel-vm/tests/ancestors.txt`.
#
# The table is a list of tiny class hierarchies and the ancestors CRuby computes
# for them. `spinel-vm` runs the same programs against its own `Classes` and has
# to agree. That makes the expectations a measurement of Ruby rather than a
# transcription of one engineer's reading of `class.c`, which matters because
# `include`/`prepend` ordering is decided by rules that CRuby documents nowhere.
#
#   ruby scripts/ancestors-oracle.rb --check      # CI: agree, or fail
#   ruby scripts/ancestors-oracle.rb --generate   # rewrite the `expect` lines
#
# The DSL is five verbs, chosen so one line maps to one line of Ruby:
#
#   module A            # A = Module.new
#   class C             # C = Class.new
#   class D < C         # D = Class.new(C)
#   include C A B       # C.include(A, B)
#   prepend C A         # C.prepend(A)
#   singleton C SC      # SC = C.singleton_class
#   expect C : C A B    # C.ancestors, keeping what this case named
#
# `expect` keeps only the entities the case declared, plus any built-in it names
# itself, so `Object`/`Kernel`/`BasicObject` stay out of the answer unless a case
# is about them.

TABLE = File.expand_path("../crates/spinel-vm/tests/ancestors.txt", __dir__)
BUILTIN = %w[BasicObject Object Kernel Comparable Enumerable Numeric
             Module Class Symbol String Integer Array Hash Proc Exception].freeze

# One case: the entities it declares, in declaration order, and the ops on them.
Case = Struct.new(:name, :line, :ops, :entities, keyword_init: true)

def parse(text)
  cases = []
  current = nil
  text.each_line.with_index(1) do |raw, lineno|
    line = raw.sub(/#.*/, "").strip
    next if line.empty?

    words = line.split
    case words.first
    when "case"
      abort "line #{lineno}: nested case" if current
      current = Case.new(name: words[1], line: lineno, ops: [], entities: [])
    when "end"
      abort "line #{lineno}: end without case" unless current
      cases << current
      current = nil
    else
      abort "line #{lineno}: #{line.inspect} outside a case" unless current
      current.ops << [lineno, words]
      current.entities << words[1] if %w[module class].include?(words.first)
      current.entities << words[2] if words.first == "singleton"
    end
  end
  abort "unterminated case #{current.name}" if current
  cases
end

# Run one case in a fresh namespace and return `[name, ancestors]` per `expect`.
def run(kase)
  env = {}
  BUILTIN.each { |name| env[name] = Object.const_get(name) }
  results = []

  kase.ops.each do |lineno, words|
    verb, *rest = words
    fetch = lambda do |name|
      env.fetch(name) { abort "line #{lineno}: #{name} is not declared" }
    end

    case verb
    when "module"
      env[rest[0]] = Module.new
    when "class"
      superclass = rest[1] == "<" ? fetch.call(rest[2]) : Object
      env[rest[0]] = Class.new(superclass)
    when "singleton"
      env[rest[1]] = fetch.call(rest[0]).singleton_class
    when "include", "prepend"
      target = fetch.call(rest[0])
      begin
        target.send(verb, *rest[1..].map { |name| fetch.call(name) })
      rescue TypeError, ArgumentError => e
        abort "line #{lineno} (#{kase.name}): #{e.class}: #{e.message}"
      end
    when "expect"
      target = fetch.call(rest[0])
      abort "line #{lineno}: expected `:`" unless rest[1] == ":"
      # Only what this case is about: everything it declared, and any built-in
      # it asked for by name.
      named = kase.entities + (rest[2..] & BUILTIN)
      keep = named.map { |name| fetch.call(name) }
      labels = env.invert
      got = target.ancestors.select { |a| keep.include?(a) }.map { |a| labels[a] }
      results << [lineno, rest[0], got]
    else
      abort "line #{lineno}: unknown verb #{verb.inspect}"
    end
  end
  results
end

mode = ARGV[0] || "--check"
abort "usage: #{$0} [--check|--generate]" unless %w[--check --generate].include?(mode)

text = File.read(TABLE)
lines = text.lines
failures = 0

parse(text).each do |kase|
  run(kase).each do |lineno, target, got|
    want = "expect #{target} : #{got.join(' ')}".rstrip
    indent = lines[lineno - 1][/\A\s*/]
    have = lines[lineno - 1].sub(/#.*/, "").strip
    next if have == want

    if mode == "--generate"
      lines[lineno - 1] = "#{indent}#{want}\n"
    else
      failures += 1
      warn "#{TABLE}:#{lineno}: #{kase.name}"
      warn "  table: #{have}"
      warn "  ruby:  #{want}"
    end
  end
end

if mode == "--generate"
  File.write(TABLE, lines.join)
  puts "wrote #{TABLE}"
elsif failures.zero?
  puts "#{TABLE} matches #{RUBY_ENGINE} #{RUBY_VERSION}"
else
  abort "#{failures} case(s) disagree with #{RUBY_ENGINE} #{RUBY_VERSION}"
end

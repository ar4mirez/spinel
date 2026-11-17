# `ripper` (#837): what RSpec asks of it — `Ripper.sexp` and `Ripper.lex`
# over a spec file, from rspec-core's snippet extractor and rspec-support's
# `Source` — and what it takes as a source. Its output must be what ruby
# 4.0.9 prints, which is `ripper.out`. The constructs where Prism's
# translation is not yet parse.y's are listed in `lib/ripper.rb`, and left
# out here.
require "ripper"
require "stringio"

SOURCES = {
  "a spec file" => <<~'RUBY',
    RSpec.describe "Arithmetic" do
      it "adds" do
        expect(1 + 1).to eq(2)
      end

      it "fails on purpose" do
        expect([1, 2, 3].map { |x| x * 2 }).to eq([2, 4, 7])
      end
    end
  RUBY
  "a failure over several lines" => <<~'RUBY',
    RSpec.describe "Multi" do
      let(:value) { { a: 1, b: [1, 2] } }

      it "fails over lines" do
        expect(
          value
        ).to eq(
          { a: 1, b: [1, 3] }
        )
      end

      it "fails in a block" do
        [1].each do |x|
          expect(x).to be > 2
        end
      end
    end
  RUBY
  "methods and literals" => <<~'RUBY',
    # A class, as a spec file's subject might be written.
    class Greeter < Struct.new(:name)
      GREETING = "hello"

      def greet(punctuation = "!", *rest, loud: false, **opts, &block)
        text = "#{GREETING}, #{name}#{punctuation}"
        loud ? text.upcase : text
      end

      private def secret; :s; end
    end

    x = %w[a b c].each_with_index.to_h { |s, i| [s, i] } rescue nil
    y = ->(n) { n ** 2 }.(3)
    printf("%d\n", 1_000) unless defined?(@ivar) || $gvar
    foo bar # trailing
  RUBY
}

SOURCES.each do |name, source|
  puts "== #{name}"
  p Ripper.sexp(source)
  p Ripper.lex(source)
  p Ripper.tokenize(source)
end

puts "== sources that are not Strings"
class Wrapped
  def initialize(string)
    @string = string
  end

  def to_str
    @string
  end
end
p Ripper.sexp(Wrapped.new("a = 1"))
p Ripper.lex(Wrapped.new("a = 1"))
p Ripper.sexp(StringIO.new("a = 1\nb\n"))
p Ripper.lex(StringIO.new("a = 1\nb\n"))
[nil, 42, true, Object.new.tap { |o| def o.gets = 42 }].each do |source|
  Ripper.sexp(source)
rescue TypeError => e
  p e
end

puts "== a syntax error"
p Ripper.sexp("def m(")

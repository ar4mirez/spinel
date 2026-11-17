# `require "prism"` (#837): the prism gem's Ruby half over Spinel's C half
# (`lib/prism/prism.rb`). Its output must be what ruby 4.0.9 prints with the
# prism 1.9.0 gem's C extension, which is `prism.out`.
require "prism"
require "tmpdir"

SOURCE = <<~'RUBY'
  # frozen_string_literal: true
  def greet(name, loud: false) = loud ? "HI #{name}" : "hi #{name}" # greeting
  =begin
  docs
  =end
  greet("you", loud: true)
RUBY

p Prism::VERSION
p Prism::BACKEND
result = Prism.parse(SOURCE)
p result.success?
puts result.value.inspect
p result.comments.map { |c| [c.class, c.slice] }
p result.magic_comments.map { |c| [c.key, c.value] }
p result.data_loc
p Prism.lex(SOURCE).value.first(8).map { |token, state| [token.type, token.value, token.location.start_offset, state] }
p Prism.parse_lex(SOURCE).value[1].size
p Prism.parse_comments(SOURCE).map(&:slice)
p Prism.parse_success?(SOURCE), Prism.parse_failure?("def (")
p Prism.dump(SOURCE).byteslice(0, 8).bytes
p Prism.profile(SOURCE)

broken = Prism.parse("def m(a\n  1 +\nend", filepath: "broken.rb", line: 10)
p broken.success?
broken.errors.each { |e| p [e.type, e.message, e.location.start_line, e.level] }
p broken.value.location.start_line
p Prism.parse("x = 1; y", scopes: [[:y]]).value.statements.body.last.class
p Prism.parse("foo = 1", frozen_string_literal: true, freeze: true).value.frozen?
p Prism.parse("p 1", version: "3.3.0").success?
p Prism.parse("__END__\ndata").data_loc.slice
p Prism.parse("\xff", encoding: false).success?

Dir.mktmpdir do |dir|
  path = File.join(dir, "a.rb")
  File.write(path, "a = [1, 2]\n")
  p Prism.parse_file(path).value.statements.body.first.class
  p Prism.parse_file(path).source.source
  p Prism.lex_file(path).value.size
  p Prism.parse_file_success?(path), Prism.parse_file_failure?(path)
  p Prism.parse_file_comments(path)
end

p Prism::StringQuery.local?("foo"), Prism::StringQuery.constant?("Foo"), Prism::StringQuery.method_name?("foo=")
[
  -> { Prism.parse(1) },
  -> { Prism.parse("1", nope: 1) },
  -> { Prism.parse("1", version: "2.0") },
  -> { Prism.parse("1", command_line: "z") },
  -> { Prism.parse("1", scopes: [1]) },
  -> { Prism::StringQuery.local?(:foo) },
  -> { Prism::StringQuery.local?("foo".encode("UTF-16LE")) },
].each do |call|
  call.call
rescue StandardError => e
  p e
end

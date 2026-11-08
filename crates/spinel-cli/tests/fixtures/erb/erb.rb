# The vendored erb (#52), run as ruby 4.0.9 runs it: crates/spinel-cli/tests/erb.rs
# holds what ruby 4.0.9 prints for this script.
require "erb"

def show(label, value)
  puts "#{label}: #{value.inspect}"
end

items = %w[apple pear]
template = <<~T
  <ul>
  <% items.each do |item| %>
    <li><%= item %></li>
  <% end %>
  </ul>
T
show "plain", ERB.new(template).result(binding)
show "trim <>", ERB.new(template, trim_mode: "<>").result(binding)
show "trim >", ERB.new(template, trim_mode: ">").result(binding)
show "trim -", ERB.new("<% items.each do |i| -%>\n<%= i %>\n<% end -%>\n", trim_mode: "-").result(binding)
show "trim %", ERB.new("% items.each do |i|\n* <%= i %>\n% end\n%% literal\n", trim_mode: "%").result(binding)
show "trim %<>", ERB.new("% 2.times do |n|\n<%= n %>\n% end\n", trim_mode: "%<>").result(binding)
show "comment", ERB.new("a<%# ignored %>b").result
show "escape", ERB.new("<%% x %%> <%= 1 + 1 %>").result
show "result_with_hash", ERB.new("<%= name %> is <%= age %>").result_with_hash(name: "Ann", age: 30)
show "eoutvar src", ERB.new("x<%= 1 %>", eoutvar: "@out").src.lines.last
show "src", ERB.new("hi <%= who %>").src

erb = ERB.new("<%= raise 'boom' %>")
erb.filename = "page.erb"
erb.lineno = 10
begin
  erb.result
rescue => e
  show "location", [e.message, e.backtrace_locations.first.path, e.backtrace_locations.first.lineno]
end
erb.location = ["other.erb", 3]
show "location=", [erb.filename, erb.lineno]

class Greeter
  def initialize(name) = @name = name
end
ERB.new("Hello, <%= @name %>!").def_method(Greeter, "greet()")
show "def_method", Greeter.new("Bo").greet

klass = ERB.new("<%= a %>+<%= b %>").def_class(Object, "render(a, b)")
show "def_class", klass.new.render(1, 2)
mod = ERB.new("[<%= x %>]").def_module("wrap(x)")
show "def_module", Object.new.extend(mod).wrap(:y)

include ERB::Util
show "h", h(%q(<a href="x?a=1&b='2'">))
show "html_escape", html_escape(nil)
show "u", u("Programming Ruby:  The Pragmatic Programmer's Guide ~")
show "url_encode", url_encode("ä b")
show "Escape", ERB::Escape.html_escape(42)
show "version", ERB.version.is_a?(String)

$stdout.flush
out = +""
def out.write(s) = self << s.to_s
orig = $stdout
$stdout = out
ERB.new("ran <%= 6 * 7 %>").run(binding)
$stdout = orig
show "run", out

compiler = ERB::Compiler.new("<>")
code, enc = compiler.compile("<% if true %>\nyes\n<% end %>\n")
show "compiler", [eval(code), enc]
show "encoding", ERB.new("é".encode("UTF-8")).encoding

show "bad trim", ERB.new("<%= 1 %>", trim_mode: "z").result
show "unclosed", ERB.new("a <%= 1").result

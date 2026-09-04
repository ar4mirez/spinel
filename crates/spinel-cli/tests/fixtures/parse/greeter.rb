# frozen_string_literal: true
class Greeter
  def greet(name, greeting: "hi")
    @seen ||= {}
    @seen[name] = "#{greeting}, #{name}"
  end
end

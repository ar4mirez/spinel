# frozen_string_literal: true

module Greeting
  def self.hello(name)
    words = %w[hello there].map { |word| word.capitalize }
    "#{words.join(" ")}, #{name}! frozen=#{"literal".frozen?}"
  end

  def self.where
    [File.basename(__FILE__), __LINE__, File.basename(__dir__)]
  end
end

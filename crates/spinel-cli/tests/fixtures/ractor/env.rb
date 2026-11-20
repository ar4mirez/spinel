# ENV, which CRuby 4.0.9 shares between Ractors unfrozen (#865): shareable
# and not frozen in every Ractor, and four Ractors writing and reading it in
# parallel, each its own variables and the others', through `[]`, `fetch`,
# `key?`, `to_h`, `keys`, `delete` and `update`, with no read that sees a
# value its own Ractor did not leave there. The main Ractor sees what they
# left.
Warning[:experimental] = false

p [Ractor.shareable?(ENV), ENV.frozen?]
p Ractor.make_shareable(ENV).equal?(ENV)
p ENV.frozen?

ENV["SPINEL_ENV_MAIN"] = "main"

ractors = (1..4).map do |k|
  Ractor.new(k) do |k|
    bad = 0
    mine = "SPINEL_ENV_R#{k}"
    200.times do |i|
      value = "#{k}:#{i}"
      ENV[mine] = value
      bad += 1 unless ENV[mine] == value
      bad += 1 unless ENV.fetch(mine) == value
      bad += 1 unless ENV.key?(mine)
      bad += 1 unless ENV.to_h[mine] == value
      bad += 1 unless ENV.keys.include?(mine)
      bad += 1 unless ENV["SPINEL_ENV_MAIN"] == "main"
      other = ENV["SPINEL_ENV_R#{k % 4 + 1}"]
      bad += 1 unless other.nil? || other.start_with?("#{k % 4 + 1}:")
      gone = "SPINEL_ENV_GONE#{k}"
      ENV.update(gone => "x")
      bad += 1 unless ENV.delete(gone) == "x"
      bad += 1 unless ENV[gone].nil?
    end
    [k, bad, Ractor.shareable?(ENV), ENV.frozen?, ENV.to_s, ENV[mine]]
  end
end

ractors.each { |r| p r.value }
p (1..4).map { |k| ENV["SPINEL_ENV_R#{k}"] }
p ENV.keys.grep(/\ASPINEL_ENV_/).sort
p ENV.to_h.count { |name, _| name.start_with?("SPINEL_ENV_GONE") }

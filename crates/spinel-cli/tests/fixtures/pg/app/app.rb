# The app `crates/spinel-cli/tests/pg.rs` runs once `spinel install` has
# installed this directory's Gemfile.lock, with pg-spinel in pg's place: it
# connects to the PostgreSQL server `DATABASE_URL` names and round-trips a
# query, printing what it read back. The table is its own, made afresh.
require "bundler/setup"
require "pg"

conn = PG.connect(ENV.fetch("DATABASE_URL"))
conn.exec("SET client_min_messages TO warning")
conn.exec("DROP TABLE IF EXISTS spinel_pg_notes")
conn.exec(<<~SQL)
  CREATE TABLE spinel_pg_notes (
    id serial PRIMARY KEY,
    body text NOT NULL UNIQUE,
    created_by text NOT NULL
  )
SQL

insert = "INSERT INTO spinel_pg_notes (body, created_by) VALUES ($1, $2) RETURNING id"
res = conn.exec_params(insert, ["hello from Spinel", "pg #{PG::VERSION}"])
puts "#{res.cmd_status}: id #{res.getvalue(0, 0)}"
conn.transaction do |c|
  c.exec_params(insert, ["héllo again", "a transaction"])
end

conn.exec("SELECT id, body, created_by FROM spinel_pg_notes ORDER BY id").each do |row|
  puts "#{row["id"]}: #{row["body"]} (#{row["created_by"]})"
end

conn.prepare("note", "SELECT body FROM spinel_pg_notes WHERE id = $1")
puts conn.exec_prepared("note", [2]).getvalue(0, 0)

begin
  conn.exec_params(insert, ["hello from Spinel", "a second time"])
rescue PG::UniqueViolation => e
  puts "#{e.class}: #{e.result.error_field(PG::PG_DIAG_SQLSTATE)} on #{e.result.error_field(PG::PG_DIAG_CONSTRAINT_NAME)}"
  puts e.message.lines.first
end

puts conn.exec("SELECT count(*) FROM spinel_pg_notes").getvalue(0, 0)
conn.exec("DROP TABLE spinel_pg_notes")
conn.finish

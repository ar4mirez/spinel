# The Sinatra + SQLite app `crates/spinel-cli/tests/sqlite3.rs` serves once
# `spinel install` has installed this directory's Gemfile.lock, with
# sqlite3-spinel in sqlite3's place: on WEBrick, on 127.0.0.1 and the port
# in `PORT`, over the database at `DATABASE`.
require "bundler/setup"
require "sinatra/base"
require "sqlite3"

DB = SQLite3::Database.new(ENV.fetch("DATABASE"), results_as_hash: true)
DB.execute(<<~SQL)
  CREATE TABLE IF NOT EXISTS notes (
    id INTEGER PRIMARY KEY,
    body TEXT NOT NULL,
    created_by TEXT NOT NULL
  )
SQL

class App < Sinatra::Base
  set :server, "webrick"
  set :bind, "127.0.0.1"
  set :port, Integer(ENV.fetch("PORT"))
  set :logging, false
  set :quiet, true

  # Write a note and read every note back, in one request.
  post "/notes" do
    content_type "text/plain"
    id = DB.transaction do
      DB.execute("INSERT INTO notes (body, created_by) VALUES (?, ?)",
        [params.fetch("body"), "sqlite3 #{SQLite3::VERSION}, SQLite #{SQLite3::SQLITE_VERSION}"])
      DB.last_insert_row_id
    end
    status 201
    rows = DB.execute("SELECT id, body, created_by FROM notes ORDER BY id")
    "created #{id}\n" + rows.map { |r| "#{r["id"]}: #{r["body"]} (#{r["created_by"]})\n" }.join
  end

  get "/notes/:id" do
    content_type "text/plain"
    row = DB.get_first_row("SELECT body FROM notes WHERE id = ?", Integer(params[:id]))
    halt 404, "no note #{params[:id]}\n" unless row
    "#{row["body"]}\n"
  end
end

App.run!

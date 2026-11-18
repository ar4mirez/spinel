# What a program sees of the sqlite3 gem's Ruby API: each line one call's
# answer or the exception it raised, class, message and code. `surface.txt`
# is what CRuby 4.0 printed with upstream sqlite3 2.10.2 (`build.rb` records
# it); `crates/spinel-cli/tests/sqlite3.rs` runs this on Spinel with
# sqlite3-spinel installed in sqlite3's place and compares, line for line.
# `Module#constants` lists C-defined constants in hash order on CRuby, so
# `Open`'s are sorted.
require "sqlite3"
require "tmpdir"

def show(label)
  v = yield
  puts "#{label}: #{v.inspect}"
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.inspect} code=#{e.respond_to?(:code) ? e.code.inspect : "-"}"
end

show("VERSION") { SQLite3::VERSION }
show("SQLITE_VERSION") { [SQLite3::SQLITE_VERSION, SQLite3::SQLITE_VERSION_NUMBER, SQLite3.libversion, SQLite3.threadsafe?, SQLite3.sqlcipher?] }
show("Open") { SQLite3::Constants::Open.constants.sort.map { |c| [c, SQLite3::Constants::Open.const_get(c)] } }
show("ancestors") { SQLite3::SQLException.ancestors.take(3) }

Dir.mktmpdir do |dir|
  path = File.join(dir, "app.db")
  db = SQLite3::Database.new(path)
  show("new") { [db.class, db.closed?, db.readonly?, db.results_as_hash, db.filename == File.realpath(path)] }
  show("ivars") { db.instance_variables.sort }
  show("create") { db.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL, score REAL, data BLOB, n INTEGER)") }
  show("insert") { db.execute("INSERT INTO notes (body, score, data, n) VALUES (?, ?, ?, ?)", ["hé llo", 1.5, "\x00\xff".b, 2**40]) }
  show("last_insert_row_id") { [db.last_insert_row_id, db.changes, db.total_changes] }
  show("insert named") { db.execute("INSERT INTO notes (body, score) VALUES (:body, :score)", {"body" => "two", score: 0.1}) }
  show("insert sym") { db.execute("INSERT INTO notes (body, n) VALUES (:b, :n)", b: "three", n: nil) }
  show("bignum") { db.execute("INSERT INTO notes (body, n) VALUES (?, ?)", ["big", 2**70]); db.get_first_value("SELECT n FROM notes WHERE body = 'big'") }
  show("select") { db.execute("SELECT * FROM notes ORDER BY id") }
  show("select encodings") { db.execute("SELECT body, data FROM notes WHERE id = 1").first.map { |v| [v.encoding, v.frozen?] } }
  show("execute2") { db.execute2("SELECT id, body FROM notes WHERE id < ?", 3) }
  show("get_first_row") { db.get_first_row("SELECT body, score FROM notes WHERE id = ?", 2) }
  show("get_first_value") { db.get_first_value("SELECT count(*) FROM notes") }
  show("get_first_value none") { db.get_first_value("SELECT body FROM notes WHERE id = 99") }
  show("block") { r = []; db.execute("SELECT id FROM notes") { |row| r << row }; r }
  db.results_as_hash = true
  show("hash") { db.execute("SELECT id, body FROM notes WHERE id = 1") }
  show("hash first row") { db.get_first_row("SELECT id, body FROM notes WHERE id = 1") }
  show("hash first value") { db.get_first_value("SELECT body FROM notes WHERE id = 1") }
  db.results_as_hash = false
  show("prepare") do
    stmt = db.prepare("SELECT id, body, score FROM notes WHERE id > ? ORDER BY id")
    out = [stmt.columns, stmt.types, stmt.bind_parameter_count, stmt.sql, stmt.column_count]
    out << stmt.execute(1).to_a
    out << stmt.execute!(2)
    out << stmt.expanded_sql
    stmt.reset!
    out << stmt.step << stmt.done? << stmt.active?
    stmt.close
    out << stmt.closed?
    out
  end
  show("closed stmt") { s = db.prepare("SELECT 1"); s.close; s.step }
  show("closed stmt close") { s = db.prepare("SELECT 1"); s.close; s.close }
  show("named_params") { db.prepare("SELECT :a, $b, @c, ?, ?5") { |s| s.named_params } }
  show("remainder") { s = db.prepare("SELECT 1; SELECT 2"); [s.remainder, s.remainder.encoding] }
  show("bind no such") { db.prepare("SELECT :a") { |s| s.bind_param(:b, 1) } }
  show("bind bad type") { db.execute("SELECT ?", [Object.new.class]) }
  show("bind true") { db.execute("SELECT ?", [true]) }
  show("bind sym") { db.execute("SELECT ?", [:x]) }
  show("blob class") { db.execute("SELECT typeof(?)", [SQLite3::Blob.new("ab")]) }
  show("to_blob") { db.execute("SELECT typeof(?), typeof(?)", ["x".to_blob, "x".b]) }
  show("syntax") { db.execute("SELEC 1") }
  show("syntax sql") { begin; db.execute("SELECT * FROM\n  nope"); rescue => e; [e.class, e.sql, e.sql.encoding, e.sql_offset, e.message]; end }
  show("no table") { db.execute("SELECT * FROM missing") }
  show("constraint") { db.execute("INSERT INTO notes (body) VALUES (NULL)") }
  show("constraint code") { begin; db.execute("INSERT INTO notes (id, body) VALUES (1, 'x')"); rescue => e; [e.class, e.code, e.message, e.sql]; end }
  show("extended") { db.extended_result_codes = true; begin; db.execute("INSERT INTO notes (id, body) VALUES (1, 'x')"); rescue => e; [e.class, e.code, e.message]; ensure; db.extended_result_codes = false; end }
  show("transaction") do
    db.transaction do |t|
      t.execute("INSERT INTO notes (body) VALUES ('in tx')")
      [t.equal?(db), db.transaction_active?]
    end
  end
  show("after tx") { [db.transaction_active?, db.get_first_value("SELECT count(*) FROM notes WHERE body = 'in tx'")] }
  show("rollback") do
    begin
      db.transaction(:immediate) { db.execute("INSERT INTO notes (body) VALUES ('gone')"); raise "boom" }
    rescue => e
      [e.message, db.get_first_value("SELECT count(*) FROM notes WHERE body = 'gone'")]
    end
  end
  show("nested tx") { db.transaction { db.transaction { } } }
  show("tx no block") { [db.transaction, db.transaction_active?, db.commit, db.transaction_active?] }
  show("rollback none") { db.rollback }
  show("query") { db.query("SELECT body FROM notes WHERE id = ?", [1]) { |rs| [rs.class, rs.next, rs.next, rs.eof?, rs.columns] } }
  show("execute_batch") { db.execute_batch("CREATE TABLE t (a); INSERT INTO t VALUES (1); INSERT INTO t VALUES (2);") }
  show("execute_batch2") { db.execute_batch2("SELECT a, 1.5, NULL FROM t; SELECT 'x' AS y") }
  show("execute_batch2 hash") { db.results_as_hash = true; r = db.execute_batch2("SELECT a FROM t"); db.results_as_hash = false; r.map { |h| h.map { |k, v| [k, k.encoding, v, v&.encoding] } } }
  show("execute_batch2 err") { db.execute_batch2("SELECT nope FROM t") }
  show("pragmas") { [db.foreign_keys, db.journal_mode, db.encoding, db.user_version] }
  show("busy_timeout") { db.busy_timeout = 100; db.busy_timeout(50); true }
  show("busy_handler nil") { db.busy_handler.equal?(db) }
  show("errmsg") { [db.errmsg, db.errmsg.encoding, db.errcode] }
  show("complete?") { [db.complete?("SELECT 1;"), db.complete?("SELECT 1")] }
  show("stat") { s = db.prepare("SELECT * FROM notes"); s.execute.to_a; [s.stat.keys, s.stat(:vm_steps) > 0] }
  show("stat bad") { db.prepare("SELECT 1").stat(:nope) }
  show("float") { db.execute("SELECT 0.1 + 0.2, 1e300 * 10, -0.0, 3.0") }
  show("int64") { db.execute("SELECT 9223372036854775807, -9223372036854775808, ?", [-2**63]) }
  show("quote") { SQLite3::Database.quote("it's") }
  show("statement types") { db.prepare("SELECT n, body, 1 FROM notes") { |s| s.types } }
  show("each frozen") { db.execute("SELECT 1").frozen? }
  show("row frozen") { db.execute("SELECT 1").first.frozen? }
  show("empty sql") { db.execute("") }
  show("comment sql") { db.execute("-- nothing") }
  show("close") { db.close; [db.closed?, db.close.equal?(db)] }
  show("closed use") { db.execute("SELECT 1") }
  show("closed changes") { db.changes }
  show("readonly") do
    ro = SQLite3::Database.new(path, readonly: true)
    begin
      [ro.readonly?, ro.execute("SELECT count(*) FROM notes"), (ro.execute("DELETE FROM notes") rescue [$!.class, $!.message, $!.code])]
    ensure
      ro.close
    end
  end
  show("cantopen") { SQLite3::Database.new(File.join(dir, "no/such/dir.db")) }
  show("cantopen ro") { SQLite3::Database.new(File.join(dir, "missing.db"), readonly: true) }
  show("open block") { SQLite3::Database.open(":memory:") { |m| m.execute("SELECT 1 + 1") } }
  show("new block") { r = nil; d = SQLite3::Database.new(":memory:") { |m| r = m.get_first_value("SELECT 3") }; [r, d.closed?] }
  show("memory filename") { SQLite3::Database.new(":memory:").filename }
  show("strict") { SQLite3::Database.new(":memory:", strict: true).execute('SELECT "x"') }
  show("not strict") { SQLite3::Database.new(":memory:").execute('SELECT "x"') }
  show("default tx mode") { m = SQLite3::Database.new(":memory:", default_transaction_mode: :immediate); m.transaction { m.transaction_active? } }
  show("flags") { SQLite3::Database.new(":memory:", flags: SQLite3::Constants::Open::READWRITE | SQLite3::Constants::Open::CREATE).readonly? }
  show("version_info keys") { SQLite3::VERSION_INFO[:sqlite].keys }
  show("status") { SQLite3.status(SQLite3::Constants::Status::MEMORY_USED).keys }
  show("open bad type") { SQLite3::Database.new(nil) }
  show("utf16") do
    d = SQLite3::Database.new(File.join(dir, "u16.db").encode(Encoding::UTF_16LE))
    d.execute("CREATE TABLE x (a)"); d.execute("INSERT INTO x VALUES ('hé')")
    [d.encoding, d.execute("SELECT a FROM x")]
  end
end

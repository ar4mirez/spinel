# What a program sees of the pg gem's Ruby API: each line one call's answer
# or the exception it raised, class and message. `surface.txt` is what CRuby
# 4.0 printed with upstream pg 1.7.0 built from source against the system's
# libpq (`build.rb` records it); `crates/spinel-cli/tests/pg.rs` runs this on
# Spinel with pg-spinel installed in pg's place and compares, line for line.
#
# The server is the one `SPINEL_PG_TEST_URL` names. What depends on where it
# runs (host, port, user, pids, oids, the server's minor version) is printed
# as whether it is what it should be, and the tables are temporary, so that
# any server of the same major version prints the same.
require "pg"

URL = ENV.fetch("SPINEL_PG_TEST_URL")

# One line, an object's address masked.
def show(label)
  v = yield
  puts "#{label}: #{v.inspect}".gsub(/0x\h{8,}/, "0x")
rescue Exception => e
  puts "#{label}: #{e.class}: #{e.message.inspect}".gsub(/0x\h{8,}/, "0x")
end

def enc(value)
  value.is_a?(String) ? [value, value.encoding.to_s, value.frozen?] : value
end

show("VERSION") { [PG::VERSION, PG.library_version >= 100000, PG::IS_BINARY_GEM, PG.isthreadsafe, PG.threadsafe?] }
show("constants") { [PG::PGRES_TUPLES_OK, PG::CONNECTION_OK, PG::PQTRANS_INERROR, PG::PG_DIAG_SQLSTATE, PG::INVALID_OID, PG::Connection::PQPING_OK, PG::Result::PGRES_FATAL_ERROR] }
show("errors") { [PG::UniqueViolation.ancestors.take(5), PG::ERROR_CLASSES["23505"], PG::ERROR_CLASSES["23"], PG::ERROR_CLASSES.size] }
show("error classes") { [PG::ConnectionBad.superclass, PG::UndefinedTable.superclass, PG::SyntaxError.superclass, PG::Error.instance_method(:error).owner] }
show("res_status") { [PG::Result.res_status(PG::PGRES_COMMAND_OK), PG::Result.res_status(2).encoding.to_s] }
show("Result.new") { PG::Result.new }
show("escape_string") { [PG::Connection.escape_string("O'Reilly"), PG::Connection.escape("a\\b")] }
show("quote_ident") { [PG::Connection.quote_ident("ta\"ble"), PG::Connection.quote_ident(%w[sch tab]), PG::Connection.quote_ident("x").encoding.to_s] }
show("escape_bytea") { [PG::Connection.escape_bytea("\x00\xffab".b), PG::Connection.unescape_bytea("\\x00ff61"), PG::Connection.unescape_bytea("a\\000b")] }
show("escape errors") { PG::Connection.escape_string(nil) }
show("conninfo_parse") { PG::Connection.conninfo_parse("host=h port=1 dbname=d").select { |o| o[:val] }.map { |o| [o[:keyword], o[:val]] } }
show("conninfo_parse error") { PG::Connection.conninfo_parse("host") }
show("parse_connect_args") { PG::Connection.parse_connect_args("h", 1, nil, nil, "d", "u", "p").split(" ").reject { |s| s.start_with?("fallback") } }
show("parse_connect_args hash") { PG::Connection.parse_connect_args(dbname: "d", port: 5432).split(" ").reject { |s| s.start_with?("fallback") } }
show("allocate") { c = PG::Connection.allocate; [c.instance_variables.sort, c.finished?] }
show("allocate db") { PG::Connection.allocate.db }
show("refused") { PG.connect(host: "127.0.0.1", port: 1, connect_timeout: 5) rescue [$!.class, $!.message.encoding.to_s, $!.message.include?("Connection refused"), $!.connection.class] }

params = PG::Connection.conninfo_parse(URL).each_with_object({}) { |o, h| h[o[:keyword].to_sym] = o[:val] if o[:val] }
conn = PG.connect(URL)
show("connect url") { [conn.class, conn.status, conn.finished?, conn.transaction_status] }
show("connect hash") { PG.connect(params) { |c| [c.db == params[:dbname], c.user == params[:user], c.exec("select 1").getvalue(0, 0)] } }
show("connect string") { PG::Connection.new(PG::Connection.connect_hash_to_string(params)).finish }
show("connect positional") { PG::Connection.open(params[:host], params[:port], nil, nil, params[:dbname], params[:user], params[:password]).tap(&:finish).finished? }
show("ivars") { conn.instance_variables.sort }
show("info") { [conn.db == params[:dbname], conn.db.encoding.to_s, conn.host == params[:host], conn.port == (params[:port] || 5432).to_i, conn.user == params[:user], conn.options, conn.tty] }
show("versions") { [conn.server_version / 10000, conn.protocol_version, conn.backend_pid > 0] }
show("encoding") { [conn.get_client_encoding, conn.internal_encoding, conn.external_encoding, conn.parameter_status("server_encoding"), conn.parameter_status("nope")] }
show("inspect") { conn.inspect.sub(/host=\S+ port=\d+ user=\S+/, "host port user") }
show("socket_io") { [conn.socket_io.class, conn.socket_io.autoclose?, conn.socket_io.equal?(conn.socket_io)] }
show("type maps") { [conn.type_map_for_results.class, conn.type_map_for_queries.frozen?, conn.field_name_type, conn.encoder_for_put_copy_data] }
show("needs_password") { [conn.connection_needs_password, conn.connection_used_password, conn.ssl_in_use?] }
show("conninfo") { conn.conninfo.find { |o| o[:keyword] == "dbname" }.keys }

show("create") do
  r = conn.exec(<<~SQL)
    CREATE TEMP TABLE notes (
      id serial PRIMARY KEY, body text NOT NULL UNIQUE, score float8, data bytea,
      n bigint, flag boolean, at timestamp
    )
  SQL
  [r.class, r.result_status, r.res_status, r.cmd_status, r.cmd_tuples, r.ntuples, r.nfields, r.values, r.oid_value]
end
show("insert") do
  r = conn.exec_params("INSERT INTO notes (body, score, data, n, flag, at) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    ["hé llo", 1.5, {value: "\x00\xff".b, format: 1}, 2**40, true, "2026-11-18 12:34:56"])
  [r.cmd_status, r.cmd_tuples, r.values, enc(r.getvalue(0, 0))]
end
show("insert typed") { conn.exec_params("INSERT INTO notes (body, n) VALUES ($1, $2)", ["two", {value: "7", type: 20}]).cmd_tuples }
show("insert nil") { conn.exec_params("INSERT INTO notes (body, score) VALUES ($1, $2)", ["three", nil]).cmd_tuples }
show("insert symbol") { conn.exec_params("INSERT INTO notes (body) VALUES ($1)", [:four]).cmd_tuples }

r = conn.exec("SELECT id, body, score, data, n, flag, at, NULL AS nothing FROM notes ORDER BY id")
show("select") { [r.ntuples, r.nfields, r.cmd_tuples, r.cmd_status] }
show("fields") { [r.fields, r.fields.frozen?, r.fields.map(&:frozen?).uniq, r.fields.first.encoding.to_s] }
show("values") { r.values }
show("value encodings") { r.values.first.map { |v| enc(v) } }
show("[0]") { r[0] }
show("[3]") { r[3] }
show("each") { r.each.to_a.length }
show("each enum") { [r.each.class, r.each.size, r.each_row.first] }
show("map") { r.map { |row| row["body"] } }
show("column_values") { [r.column_values(1), r.column_values(7)] }
show("field_values") { [r.field_values("score"), r.field_values(:n)] }
show("tuple_values") { r.tuple_values(1) }
show("getvalue") { [r.getvalue(0, 1), r.getvalue(1, 2), r.getvalue(0, 5), r.getvalue(0, 6)] }
show("getisnull") { [r.getisnull(0, 0), r.getisnull(1, 2), r.getisnull(0, 7)] }
show("getlength") { [r.getlength(0, 1), r.getlength(0, 3), r.getlength(1, 2)] }
show("fname") { [r.fname(0), r.fname(7), r.fname(0).frozen?] }
show("fnumber") { [r.fnumber("body"), r.fnumber("nothing")] }
show("ftype") { (0...r.nfields).map { |i| r.ftype(i) } }
show("fmod") { (0...r.nfields).map { |i| r.fmod(i) } }
show("fsize") { (0...r.nfields).map { |i| r.fsize(i) } }
show("fformat") { (0...r.nfields).map { |i| r.fformat(i) } }
show("ftable") { [r.ftable(0) > 0, r.ftable(7), r.ftablecol(1), r.ftablecol(7)] }
show("misc") { [r.nparams, r.binary_tuples, r.oid_value, r.cleared?, r.autoclear?, r.error_message, r.error_field(PG::PG_DIAG_SQLSTATE)] }
show("result inspect") { r.inspect }
show("getvalue range") { r.getvalue(9, 0) }
show("getvalue field range") { r.getvalue(0, 99) }
show("getvalue type") { r.getvalue("0", 0) }
show("[] range") { r[99] }
show("fnumber unknown") { r.fnumber("zz") }
show("fnumber type") { r.fnumber(:body) }
show("fname range") { r.fname(-1) }
show("ftype range") { r.ftype(8) }
show("fmod range") { r.fmod(8) }
show("ftable range") { r.ftable(8) }
show("column_values range") { r.column_values(8) }
show("field_values unknown") { r.field_values("zz") }
show("tuple_values range") { r.tuple_values(4) }

t = r.tuple(0)
show("tuple") { [t.class, t.length, t.size, t.keys, t.index("body"), t.index("zz")] }
show("tuple values") { [t["body"], t[1], t[-1], t[99], t["zz"], t.fetch("id"), t.fetch(1), t.fetch("zz", :default)] }
show("tuple fetch missing") { t.fetch("zz") }
show("tuple fetch range") { t.fetch(42) }
show("tuple each") { t.each.to_a.first(2) }
show("tuple each_value") { t.each_value.to_a.length }
show("tuple inspect") { r.tuple(1).inspect }
show("tuple values all") { r.tuple(2).values }
show("tuple has_key") { [t.has_key?("body"), t.key?("zz")] }
show("tuple marshal") { u = Marshal.load(Marshal.dump(r.tuple(1))); [u.class, u.values, u.keys] }
show("each_tuple") { r.each_tuple.map { |tup| tup["id"] } }
show("tuple empty") { PG::Tuple.new.length }

show("binary") do
  b = conn.exec_params("SELECT 1::int4 AS i, 'x'::text AS t, '\\x00ff'::bytea AS b", [], 1)
  [b.binary_tuples, b.fformat(0), b.values.first.map { |v| enc(v) }]
end
show("frozen result") { f = conn.exec("SELECT 1 AS x"); f.freeze; [f.frozen?, f.values, f[0], f.tuple(0)["x"]] }
show("frozen clear") { f = conn.exec("SELECT 1 AS x"); f.freeze; f.clear }
show("frozen field_name_type") { f = conn.exec("SELECT 1 AS x"); f.freeze; f.field_name_type = :symbol }

show("prepare") { p1 = conn.prepare("ins", "INSERT INTO notes (body, score) VALUES ($1, $2)"); [p1.result_status, p1.cmd_status, p1.ntuples] }
show("exec_prepared") { conn.exec_prepared("ins", ["five", 5.5]).cmd_tuples }
show("describe_prepared") { d = conn.describe_prepared("ins"); [d.nparams, d.paramtype(0), d.paramtype(1), d.nfields] }
show("prepare typed") { conn.prepare("sel", "SELECT $1::int + 1 AS v", [23]); conn.exec_prepared("sel", [41]).values }
show("prepare again") { conn.prepare("ins", "SELECT 1") }
show("exec_prepared unknown") { conn.exec_prepared("nope", []) }
show("exec_prepared block") { conn.exec_prepared("sel", [1]) { |res| [res.values, res.cleared?] } }

show("unique violation") do
  conn.exec_params("INSERT INTO notes (body) VALUES ($1)", ["two"])
rescue PG::Error => e
  res = e.result
  [e.class, e.message, e.message.encoding.to_s, e.error, e.connection.equal?(conn), res.class, res.result_status,
   res.error_field(PG::PG_DIAG_SQLSTATE), res.error_field(PG::PG_DIAG_CONSTRAINT_NAME), res.error_field(PG::PG_DIAG_TABLE_NAME),
   res.error_field(PG::PG_DIAG_SEVERITY), res.error_field(PG::PG_DIAG_MESSAGE_PRIMARY), res.error_field(PG::PG_DIAG_MESSAGE_DETAIL),
   res.error_field(PG::PG_DIAG_MESSAGE_HINT), res.error_message == e.message,
   res.verbose_error_message(PG::PQERRORS_TERSE, PG::PQSHOW_CONTEXT_NEVER)]
end
show("not null") { conn.exec("INSERT INTO notes (body) VALUES (NULL)") }
show("syntax error") { conn.exec("SELEC 1") }
show("syntax error position") { conn.exec("SELEC 1") rescue $!.result.error_field(PG::PG_DIAG_STATEMENT_POSITION) }
show("undefined table") { conn.exec("SELECT * FROM nope") }
show("undefined column") { conn.exec_params("SELECT zz FROM notes WHERE id = $1", [1]) }
show("division by zero") { conn.exec("SELECT 1/0") }
show("invalid text") { conn.exec_params("SELECT $1::int", ["abc"]) }
show("too few params") { conn.exec_params("SELECT $1::int, $2::int", [1]) }
show("null byte") { conn.exec("SELECT '\0'") }
show("null byte param") { conn.exec_params("SELECT $1", ["a\0b"]) }
show("params type") { conn.exec_params("SELECT $1", "x") }
show("after error") { [conn.transaction_status, conn.exec("SELECT count(*) FROM notes").getvalue(0, 0)] }

show("transaction commit") { conn.transaction { |c| c.exec("INSERT INTO notes (body) VALUES ('six')"); c.transaction_status } }
show("transaction rollback") { conn.transaction { |c| c.exec("INSERT INTO notes (body) VALUES ('seven')"); raise PG::RollbackTransaction } }
show("transaction raise") { conn.transaction { |c| c.exec("INSERT INTO notes (body) VALUES ('eight')"); raise ArgumentError, "boom" } }
show("transaction error") { conn.transaction { |c| c.exec("INSERT INTO notes (body) VALUES ('two')") } }
show("after transactions") { [conn.transaction_status, conn.exec("SELECT body FROM notes ORDER BY id").column_values(0)] }
show("in error") { conn.exec("BEGIN"); conn.exec("SELECT 1/0") rescue nil; s = conn.transaction_status; conn.exec("ROLLBACK"); [s, conn.transaction_status] }

show("escape") { [conn.escape_string("O'Reilly"), conn.escape("x"), conn.escape_string("é").encoding.to_s] }
show("escape_literal") { [conn.escape_literal("it's"), conn.escape_literal("back\\slash"), conn.escape_literal("é").encoding.to_s] }
show("escape_identifier") { [conn.escape_identifier("My \"Table\""), conn.quote_ident("a.b"), conn.quote_ident(%w[public notes])] }
show("escape_bytea") { [conn.escape_bytea("\x00\x01\xff".b), conn.unescape_bytea("\\x0001ff")] }
show("escape type") { conn.escape_literal(1) }
show("embed_params") { conn.embed_params("SELECT $1, $2", ["it's", nil]) }

show("exec block") { conn.exec("SELECT 1 AS one") { |res| [res.values, res.class] } }
show("exec block cleared") { kept = nil; conn.exec("SELECT 1") { |res| kept = res }; [kept.cleared?, (kept.values rescue $!.class)] }
show("cleared") { c = conn.exec("SELECT 1"); c.clear; [c.cleared?, c.inspect] }
show("cleared access") { c = conn.exec("SELECT 1"); c.clear; c.ntuples }
show("cleared values") { c = conn.exec("SELECT 1"); c.clear; c.values }
show("empty query") { e = conn.exec(""); [e.result_status, e.res_status, e.cmd_status, e.ntuples] }
show("multi statement") { conn.exec("SELECT 1; SELECT 2 AS two").values }
show("make_empty_pgresult") { e = conn.make_empty_pgresult(PG::PGRES_COMMAND_OK); [e.result_status, e.ntuples] }
show("make_empty_pgresult error") { conn.make_empty_pgresult(PG::PGRES_FATAL_ERROR) }

show("field_name_type") { s = conn.exec("SELECT 1 AS one, 2 AS two"); s.field_name_type = :symbol; [s.field_name_type, s.fields, s[0], s.fname(1), s.tuple(0)[:one]] }
show("field_name_type conn") { conn.field_name_type = :symbol; f = conn.exec("SELECT 1 AS one").fields; conn.field_name_type = :string; f }
show("field_name_type late") { s = conn.exec("SELECT 1 AS one"); s.fields; s.field_name_type = :symbol }
show("field_name_type bad") { conn.field_name_type = :other }

show("send_query") { conn.send_query("SELECT 1 AS a; SELECT 2 AS b"); [conn.get_result.values, conn.get_result.values, conn.get_result] }
show("send_query_params") { conn.send_query_params("SELECT $1::int * 2", [21]); [conn.is_busy.class, conn.get_last_result.values, conn.get_result] }
show("send_prepare") { conn.send_prepare("s2", "SELECT $1::text"); conn.get_last_result.result_status }
show("send_query_prepared") { conn.send_query_prepared("s2", ["x"]); conn.block; [conn.is_busy, conn.get_result.values, conn.get_result] }
show("get_result block") { conn.send_query("SELECT 1"); v = conn.get_result { |res| res.values }; conn.get_result; [v.class, v.cleared?] }
show("sync_exec") { [conn.sync_exec("SELECT 1").values, conn.sync_exec_params("SELECT $1::int", [3]).values, conn.async_exec("SELECT 2").values, conn.query("SELECT 4").values] }
show("discard_results") { conn.send_query("SELECT pg_sleep(0.01)"); [conn.discard_results, conn.exec("SELECT 5").values] }
show("single row mode") { conn.send_query("SELECT generate_series(1, 3) AS n"); conn.set_single_row_mode; rows = []; conn.get_result.stream_each { |row| rows << row }; [rows, conn.get_result] }
show("single row stream_each_row") { conn.send_query("SELECT generate_series(1, 2) AS n, 'x' AS s"); conn.set_single_row_mode; rows = []; conn.get_result.stream_each_row { |row| rows << row }; [rows, conn.get_result] }
show("single row stream_each_tuple") { conn.send_query("SELECT generate_series(1, 2) AS n"); conn.set_single_row_mode; rows = []; conn.get_result.stream_each_tuple { |tup| rows << tup }; [rows.map(&:values), rows.map { |tup| tup["n"] }, conn.get_result] }
show("stream not single") { conn.exec("SELECT 1").stream_each { } }
show("nonblocking") { [conn.isnonblocking, conn.nonblocking?, conn.sync_isnonblocking] }
show("flush") { [conn.flush, conn.sync_flush] }
show("setnonblocking bad") { conn.sync_setnonblocking(1) }

show("copy in") do
  conn.copy_data("COPY notes (body, n) FROM STDIN") do
    conn.put_copy_data("copied\t1\n")
    conn.put_copy_data("copied two\t\\N\n")
  end.then { |res| [res.result_status, res.cmd_tuples] }
end
show("copy out") do
  rows = []
  conn.copy_data("COPY (SELECT body, n FROM notes WHERE body LIKE 'copied%' ORDER BY id) TO STDOUT") do
    while (row = conn.get_copy_data)
      rows << enc(row)
    end
  end
  rows
end
show("copy in error") { conn.copy_data("COPY notes (n) FROM STDIN") { conn.put_copy_data("notanumber\n") } }
show("copy not copy") { conn.copy_data("SELECT 1") { } }

show("listen") do
  conn.exec("LISTEN chan")
  conn.exec("NOTIFY chan, 'payload'")
  n = conn.notifies
  [n.keys, n[:relname], enc(n[:extra]), n[:be_pid] == conn.backend_pid, conn.notifies]
end
show("wait_for_notify") do
  conn.exec("NOTIFY chan, 'second'")
  got = nil
  name = conn.wait_for_notify(5) { |relname, pid, extra| got = [relname, pid == conn.backend_pid, extra] }
  [name, got, conn.wait_for_notify(0.05)]
end

show("large object") do
  conn.transaction do
    oid = conn.lo_creat(PG::INV_READ | PG::INV_WRITE)
    fd = conn.lo_open(oid, PG::INV_READ | PG::INV_WRITE)
    written = conn.lo_write(fd, "hello large")
    conn.lo_lseek(fd, 6, PG::SEEK_SET)
    part = conn.lo_read(fd, 5)
    tell = conn.lo_tell(fd)
    conn.lo_close(fd)
    conn.lo_unlink(oid)
    [oid > 0, written, part, part.encoding.to_s, tell]
  end
end

show("client encoding") do
  conn.set_client_encoding("LATIN1")
  v = conn.exec("SELECT 'é' AS e").getvalue(0, 0)
  out = [conn.get_client_encoding, conn.internal_encoding, enc(v), v.bytes]
  conn.internal_encoding = "UTF-8"
  out + [conn.get_client_encoding, conn.internal_encoding]
end
show("client encoding bad") { conn.set_client_encoding("NOPE") }
show("client encoding sync") { conn.sync_set_client_encoding("UTF8"); conn.internal_encoding }

show("encrypt_password") { [PG::Connection.encrypt_password("pw", "user"), conn.encrypt_password("pw", "user", "md5")] }
show("ping") { [PG::Connection.ping(URL), PG::Connection.ping(host: "127.0.0.1", port: 1)] }
show("reset") { conn.exec("SET application_name = 'before'"); conn.reset; conn.exec("SHOW application_name").getvalue(0, 0) == "before" }
show("after reset") { [conn.status, conn.exec("SELECT 1").values, conn.transaction_status] }
show("set_error_verbosity") { [conn.set_error_verbosity(PG::PQERRORS_VERBOSE), conn.set_error_verbosity(PG::PQERRORS_DEFAULT)] }
show("cancel") { conn.cancel }

show("finish") { conn.finish }
show("finished") { [conn.finished?, conn.inspect] }
show("finish again") { conn.finish }
show("exec closed") { conn.exec("SELECT 1") }
show("db closed") { conn.db }
show("status closed") { conn.status }
show("error connection") { begin; conn.status; rescue => e; e.connection.equal?(conn); end }
show("frozen") { c = PG.connect(URL); c.freeze; c.exec("SELECT 1") rescue [$!.class, $!.message.sub(/host=\S+ port=\d+ user=\S+/, "host port user")] }

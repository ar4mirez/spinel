"""The gem server `bundler.sh` runs Bundler 4.0.20 against (#89), as
`crates/spinel-cli/tests/private_source.rs` serves it to Spinel:

    python3 server.py <compact|legacy> <port file> [user:password]

`compact` serves `compact/` (`/versions`, `/info/<gem>`); `legacy` answers
404 to those and to the dependency API and serves `legacy/` (`specs.4.8.gz`,
`prerelease_specs.4.8.gz`, `quick/Marshal.4.8/`). Both serve `/gems/<file>`
from `../install/gems/`. With `user:password`, every request without that
Basic credential gets a 401. It binds 127.0.0.1 on a free port and writes
the port to the file named.
"""

import base64
import http.server
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
MODE = sys.argv[1]
AUTH = sys.argv[3] if len(sys.argv) > 3 else None


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if AUTH is not None:
            expected = "Basic " + base64.b64encode(AUTH.encode()).decode()
            if self.headers.get("Authorization") != expected:
                return self.reply(401, b"unauthorized")
        path = self.path.split("?")[0].lstrip("/")
        if ".." in path.split("/"):
            return self.reply(404, b"not found")
        if path.startswith("gems/"):
            file = os.path.join(HERE, "../install", path)
        elif MODE == "compact":
            file = os.path.join(HERE, "compact", path)
        else:
            file = os.path.join(HERE, "legacy", path)
        if not os.path.isfile(file):
            return self.reply(404, b"not found")
        with open(file, "rb") as f:
            self.reply(200, f.read())

    def reply(self, status, body):
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(sys.argv[2], "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()

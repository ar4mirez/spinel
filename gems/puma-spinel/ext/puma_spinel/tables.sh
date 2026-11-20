#!/usr/bin/env bash
#
# Write the HTTP parser's tables from upstream Puma's grammar:
#
#   gems/puma-spinel/ext/puma_spinel/tables.sh           write src/parser/tables.rs
#   gems/puma-spinel/ext/puma_spinel/tables.sh --check   fail if it differs, or if
#                                                         the grammar is not the C's
#
# Ragel 6.10 (on PATH, or $RAGEL) makes `-T0` tables of the `puma_parser`
# machine in `ragel/http11_parser.rl`, and they become Rust constants. The
# check also has Ragel make the `-G2` code upstream ships and compares it
# with `ragel/http11_parser.c`, but for its `#line` directives, so the
# tables are of the machine upstream's extension runs. `ragel/` is
# upstream's, vendored by `gems/puma-spinel/vendor.sh`. Ruby (`ruby` or
# `spinel`) turns the tables into Rust.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ragel="${RAGEL:-ragel}"
ruby="${RUBY:-ruby}"
out="$here/src/parser/tables.rs"

version="$("$ragel" --version 2>/dev/null || true)"
if [[ "$version" != *"version 6.10 "* ]]; then
  echo "tables.sh: needs Ragel 6.10 as \`$ragel\` (set RAGEL)" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cp "$here/ragel/"*.rl "$tmp/"
"$ragel" "$tmp/http11_parser.rl" -C -T0 -I "$tmp" -o "$tmp/t0.c"

"$ruby" - "$tmp/t0.c" "$tmp/tables.rs" <<'RUBY'
source, target = ARGV
c = File.read(source)
tables = [
  ["actions", "u8"], ["key_offsets", "u16"], ["trans_keys", "u8"],
  ["single_lengths", "u8"], ["range_lengths", "u8"], ["index_offsets", "u16"],
  ["indicies", "u8"], ["trans_targs", "u8"], ["trans_actions", "u8"]
]
rust = +<<~HEAD
  //! Written by `tables.sh` from upstream Puma 8.1.2's
  //! `ext/puma_http11/http11_parser.rl`: Ragel 6.10's `-T0` tables for the
  //! `puma_parser` machine, the same machine as upstream's `-G2`
  //! `http11_parser.c`. Do not edit; run `tables.sh` again.
HEAD
tables.each do |name, type|
  body = c[/_puma_parser_#{name}\[\] = \{(.*?)\};/m, 1] or abort "tables.sh: Ragel wrote no #{name}"
  values = body.split(",").map(&:strip).reject(&:empty?).map { |v| Integer(v) }
  limit = type == "u8" ? 256 : 65_536
  abort "tables.sh: #{name} has a value out of #{type}" unless values.all? { |v| v >= 0 && v < limit }
  rows = values.each_slice(16).map { |row| "    #{row.join(", ")}," }
  rust << "\n#[rustfmt::skip]\npub const #{name.upcase}: [#{type}; #{values.size}] = [\n#{rows.join("\n")}\n];\n"
end
File.write(target, rust)
RUBY

if [[ "${1:-}" == "--check" ]]; then
  status=0
  "$ragel" "$tmp/http11_parser.rl" -C -G2 -I "$tmp" -o "$tmp/g2.c"
  if ! diff -q <(grep -v '^#line' "$tmp/g2.c") <(grep -v '^#line' "$here/ragel/http11_parser.c") >/dev/null; then
    echo "tables.sh: Ragel's -G2 code from the grammar is not upstream's http11_parser.c" >&2
    status=1
  fi
  if ! cmp -s "$tmp/tables.rs" "$out"; then
    echo "tables.sh: src/parser/tables.rs is not what Ragel makes of the grammar" >&2
    status=1
  fi
  [[ $status -eq 0 ]] && echo "src/parser/tables.rs is Ragel 6.10's -T0 machine of upstream's grammar, the machine of its http11_parser.c"
  exit $status
fi

cp "$tmp/tables.rs" "$out"
echo "wrote $out"

//! The vendored `erb` (#52): every trim mode (`<>`, `>`, `-`, `%`, `%<>`),
//! comments and `<%%` escapes, `result`, `result_with_hash`, `run`, `src`
//! and `eoutvar:`, `filename`/`lineno`/`location` in a raised backtrace,
//! `def_method`, `def_class`, `def_module`, `ERB::Util` and `ERB::Escape`,
//! `ERB::Compiler`, and the invalid trim mode warning. ruby/spec's
//! `library/erb` passes 53 of 53; this covers what its examples do not.
//! Expected output is what ruby 4.0.9 prints for the same script.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

const EXPECTED: &str = r##"plain: "<ul>\n\n  <li>apple</li>\n\n  <li>pear</li>\n\n</ul>\n"
trim <>: "<ul>\n  <li>apple</li>\n  <li>pear</li>\n</ul>\n"
trim >: "<ul>\n  <li>apple</li>\n  <li>pear</li>\n</ul>\n"
trim -: "apple\npear\n"
trim %: "* apple\n* pear\n% literal\n"
trim %<>: "01"
comment: "ab"
escape: "<% x %%> 2"
result_with_hash: "Ann is 30"
eoutvar src: "@out = +''; @out.<< \"x\".freeze; @out.<<(( 1 ).to_s); @out"
src: "#coding:UTF-8\n_erbout = +''; _erbout.<< \"hi \".freeze; _erbout.<<(( who ).to_s); _erbout"
location: ["boom", "page.erb", 11]
location=: ["other.erb", 3]
def_method: "Hello, Bo!"
def_class: "1+2"
def_module: "[y]"
h: "&lt;a href=&quot;x?a=1&amp;b=&#39;2&#39;&quot;&gt;"
html_escape: ""
u: "Programming%20Ruby%3A%20%20The%20Pragmatic%20Programmer%27s%20Guide%20~"
url_encode: "%C3%A4%20b"
Escape: "42"
version: true
run: "ran 42"
yes
compiler: [nil, #<Encoding:UTF-8>]
encoding: #<Encoding:UTF-8>
bad trim: "1"
unclosed: "a  1"
"##;

const WARNING: &str = "erb.rb:73: warning: Invalid ERB trim mode: \"z\" (trim_mode: nil, 0, 1, 2, or String composed of '%' and/or '-', '>', '<>')\n";

#[test]
fn erb_renders_as_ruby_does() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/erb");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg("erb.rb")
        .current_dir(&dir)
        .output()
        .expect("spinel runs");

    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
    assert_eq!(String::from_utf8_lossy(&output.stderr), WARNING);
}

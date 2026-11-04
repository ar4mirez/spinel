//! How a `String#index` or `String#byteindex` whose match is at the start
//! grows with the String it searches, on the `spinel` binary: each call #617
//! found costing the String's length, because it tabulated or copied every
//! byte, takes about as long on a String a hundred times as long.
//!
//! ruby/spec has no example for how long a search takes, so the check is
//! here. Before #617 `bench/string_index.rb` put them at about 70 and 65;
//! constant is 1.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

// Room for a loaded machine above 1, well short of what a walk of the whole
// String costs.
const BOUND: f64 = 5.0;

fn ratios() -> (Vec<(String, f64)>, String) {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/string/index_constant.rb");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .output()
        .expect("spinel runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let ratios: Vec<(String, f64)> = stdout
        .lines()
        .map(|line| {
            let (call, ratio) = line.split_once(' ').expect("a call and its ratio");
            (call.to_string(), ratio.parse().expect("a ratio"))
        })
        .collect();
    assert_eq!(ratios.len(), 2, "{stdout}");
    (ratios, stdout)
}

#[test]
fn a_search_matching_at_the_start_does_not_grow_with_the_string() {
    let (first, stdout) = ratios();
    let slow: Vec<&str> = first
        .iter()
        .filter(|(_, ratio)| *ratio >= BOUND)
        .map(|(call, _)| call.as_str())
        .collect();
    if slow.is_empty() {
        return;
    }
    // A pause during one run can push a call over the bound, so a call fails
    // only if it is over it on a second run too.
    let (second, again) = ratios();
    for (call, ratio) in &second {
        assert!(
            !(slow.contains(&call.as_str()) && *ratio >= BOUND),
            "{call}: a String a hundred times as long took {ratio} times as long, twice\n{stdout}{again}"
        );
    }
}

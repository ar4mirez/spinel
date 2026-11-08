//! How the time draining or filling an `Array` from the front takes grows
//! with the `Array`, on the `spinel` binary: each pattern in #733, where every
//! `shift` and `unshift` copied the rest of the array, takes about eight times
//! as long for eight times the elements.
//!
//! ruby/spec has no example for how long a shift takes, so the check is
//! here. Before #733 the fill-then-shift ratio was about 67, and the other patterns
//! did not finish in five minutes; linear is 8.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

// Room for a loaded machine above 8, well short of what copying the rest of
// the array on every shift costs.
const BOUND: f64 = 16.0;

const PATTERNS: usize = 5;

fn ratios() -> (Vec<(String, f64)>, String) {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/array/shift_linear.rb");
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
            let (pattern, ratio) = line.split_once(' ').expect("a pattern and its ratio");
            (pattern.to_string(), ratio.parse().expect("a ratio"))
        })
        .collect();
    assert_eq!(ratios.len(), PATTERNS, "{stdout}");
    (ratios, stdout)
}

#[test]
fn shifting_an_array_grows_linearly_with_its_size() {
    let (first, stdout) = ratios();
    let slow: Vec<&str> = first
        .iter()
        .filter(|(_, ratio)| *ratio >= BOUND)
        .map(|(pattern, _)| pattern.as_str())
        .collect();
    if slow.is_empty() {
        return;
    }
    // A pause during one run can push a pattern over the bound, so a pattern
    // fails only if it is over it on a second run too.
    let (second, again) = ratios();
    for (pattern, ratio) in &second {
        assert!(
            !(slow.contains(&pattern.as_str()) && *ratio >= BOUND),
            "{pattern}: eight times the elements took {ratio} times as long, twice\n{stdout}{again}"
        );
    }
}

//! How the time filling a `Hash` takes grows with the `Hash`, on the `spinel`
//! binary: each example in #729, where every store scanned the keys already
//! there, takes about eight times as long for eight times the keys.
//!
//! ruby/spec has no example for how long a store takes, so the check is
//! here. Before #729 the ratios were about 55, 62 and 37; linear is 8.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

// Room for a loaded machine above 8, well short of what a scan of every key
// costs.
const BOUND: f64 = 16.0;

fn ratios() -> (Vec<(String, f64)>, String) {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hash/insert_linear.rb");
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
            let (fill, ratio) = line.split_once(' ').expect("a fill and its ratio");
            (fill.to_string(), ratio.parse().expect("a ratio"))
        })
        .collect();
    assert_eq!(ratios.len(), 3, "{stdout}");
    (ratios, stdout)
}

#[test]
fn filling_a_hash_grows_linearly_with_its_size() {
    let (first, stdout) = ratios();
    let slow: Vec<&str> = first
        .iter()
        .filter(|(_, ratio)| *ratio >= BOUND)
        .map(|(fill, _)| fill.as_str())
        .collect();
    if slow.is_empty() {
        return;
    }
    // A pause during one run can push a fill over the bound, so a fill fails
    // only if it is over it on a second run too.
    let (second, again) = ratios();
    for (fill, ratio) in &second {
        assert!(
            !(slow.contains(&fill.as_str()) && *ratio >= BOUND),
            "{fill}: eight times the keys took {ratio} times as long, twice\n{stdout}{again}"
        );
    }
}

//! How a `gets` loop's time grows with the file, on the `spinel` binary: each
//! read #585 found quadratic, because every search of the read buffer cost
//! the whole buffer, takes about eight times as long on a file eight times as
//! long.
//!
//! ruby/spec has no example for how long a read takes, so the check is here.
//! Before #585 the ratios were about 25, 15, 16 and 14; linear is 8.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

// Room for a loaded machine above 8, short of what a search of the whole
// buffer costs.
const BOUND: f64 = 12.0;

fn ratios() -> (Vec<(String, f64)>, String) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/io/gets_linear.rb");
    let dir = std::env::temp_dir().join(format!("spinel-io-gets-linear-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .arg(&dir)
        .output()
        .expect("spinel runs");
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let ratios: Vec<(String, f64)> = stdout
        .lines()
        .map(|line| {
            let (read, ratio) = line.split_once(' ').expect("a read and its ratio");
            (read.to_string(), ratio.parse().expect("a ratio"))
        })
        .collect();
    assert_eq!(ratios.len(), 4, "{stdout}");
    (ratios, stdout)
}

#[test]
fn a_gets_loop_grows_linearly_with_the_file() {
    let (first, stdout) = ratios();
    let slow: Vec<&str> = first
        .iter()
        .filter(|(_, ratio)| *ratio >= BOUND)
        .map(|(read, _)| read.as_str())
        .collect();
    if slow.is_empty() {
        return;
    }
    // A pause during one run can push a read over the bound, so a read fails
    // only if it is over it on a second run too.
    let (second, again) = ratios();
    for (read, ratio) in &second {
        assert!(
            !(slow.contains(&read.as_str()) && *ratio >= BOUND),
            "{read}: eight times the file took {ratio} times as long, twice\n{stdout}{again}"
        );
    }
}

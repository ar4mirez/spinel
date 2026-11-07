//! The vendored `time` (#50): formatting with `iso8601`, `xmlschema`,
//! `rfc2822` and `httpdate`, and `Time.parse`, `strptime`, `rfc2822`,
//! `httpdate`, `xmlschema` and `zone_offset`, with their errors.
//! ruby/spec's `library/time` has only seven examples, so this covers the
//! rest. Expected output is what ruby 4.0.9 prints for the same script under
//! `TZ=Asia/Kolkata`, a zone with a half-hour offset and no DST.

use std::path::PathBuf;
use std::process::Command;

const SPINEL: &str = env!("CARGO_BIN_EXE_spinel");

const EXPECTED: &str = r#"iso8601: "2024-02-29T13:05:09Z"
xmlschema 3: "2024-02-29T13:05:09.123Z"
rfc2822: "Thu, 29 Feb 2024 13:05:09 -0000"
httpdate: "Thu, 29 Feb 2024 13:05:09 GMT"
rfc822: "Thu, 29 Feb 2024 13:05:09 -0000"
local xmlschema: "2024-07-04T01:02:03+05:30"
local rfc2822: "Thu, 04 Jul 2024 00:00:00 +0530"
parse offset: 2024-02-29 13:05:09 +0900
parse rfc: 2024-02-29 13:05:09 +0000
parse now: 2020-01-01 12:30:00 +0530
parse local: "2024-12-01 10:00:00 +0530"
parse local offset: 19800
parse usec: 500000
parse to_a: [0, 0, 0, 3, 2, 2001, 6, 34, false, "IST"]
parse EST: 2024-06-01 10:00:00 -0500
parse PDT: -25200
parse leap second: 2000-01-01 00:00:00 UTC
parse utc?: false
parse garbage: ArgumentError: no time information in "garbage"
strptime z: 2024-03-01 10:11:12 +0100
strptime s: 2023-11-15 03:43:20 +0530
strptime dmy: 2024-02-29 00:00:00 +0530
strptime pm: 2024-07-04 13:02:00 +0530
strptime j: 2024-07-04 00:00:00 +0530
strptime N: 789000000
strptime bad: ArgumentError: invalid date or strptime format - 'x' '%Y'
rfc2822 parse: 2024-02-29 13:05:09 -0500
rfc2822 UT: 2023-01-01 00:00:00 UTC
rfc2822 short: 2023-01-01 00:00:00 -0500
httpdate parse: 2024-02-29 13:05:09 UTC
httpdate bad: ArgumentError: not RFC 2616 compliant date: "nope"
xmlschema parse: 2024-02-29 13:05:09.25 +0200
xmlschema date: ArgumentError: invalid xmlschema format: "2024-07-04"
iso8601 parse: 2024-02-29 13:05:09 UTC
iso8601 bad: ArgumentError: invalid xmlschema format: "2024"
zone_offset: [-18000, 19800, nil]
to_date: #<Date: 2024-01-01 ((2460311j,0s,0n),+0s,2299161j)>
to_datetime: #<DateTime: 2024-01-01T00:00:00+00:00 ((2460311j,0s,0n),+0s,2299161j)>
to_time: true
"#;

#[test]
fn time_parses_and_formats_as_ruby_does() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/time/time.rb");
    let output = Command::new(SPINEL)
        .arg("run")
        .arg(&script)
        .env("TZ", "Asia/Kolkata")
        .output()
        .expect("spinel runs");

    assert!(
        output.status.success(),
        "spinel failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED);
}

//! The legacy index (#89): what a gem server without a compact index
//! serves, read into the compact index's shape so the resolver reads it as
//! it reads any `/info` file.
//!
//! `specs.4.8.gz` and `prerelease_specs.4.8.gz` list every gem's versions, a
//! gzipped Marshal of `[name, Gem::Version, platform]` triples, as
//! `Bundler::Fetcher::Index` reads them (`fetch_all_remote_specs`). They say
//! nothing of dependencies: each version's `Gem::Specification` is
//! `quick/Marshal.4.8/<full name>.gemspec.rz`, a deflated Marshal, which
//! `RemoteSpecification#_remote_specification` fetches. Its runtime
//! dependencies and its Ruby and RubyGems requirements make the version's
//! `/info` line; there is no checksum, as Bundler has none for such a gem
//! until it is downloaded.

use std::io::Read as _;

use crate::marshal::{self, Value};

/// One entry of `specs.4.8.gz`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tuple {
    pub name: String,
    pub version: String,
    /// `ruby`, or the platform a build is for.
    pub platform: String,
}

impl Tuple {
    /// `Gem::NameTuple#full_name`: the platform after the version unless it
    /// is `ruby`. The `quick/` file of this build is named by it.
    pub fn full_name(&self) -> String {
        if self.platform.is_empty() || self.platform == "ruby" {
            format!("{}-{}", self.name, self.version)
        } else {
            format!("{}-{}-{}", self.name, self.version, self.platform)
        }
    }

    /// The first field of its `/info` line: `version[-platform]`.
    pub fn index_version(&self) -> String {
        if self.platform.is_empty() || self.platform == "ruby" {
            self.version.clone()
        } else {
            format!("{}-{}", self.version, self.platform)
        }
    }

    /// Whether every part can name a file: a server's answer is not trusted
    /// to stay inside the cache.
    fn safe(&self) -> bool {
        let ok = |s: &str, extra: &[u8]| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || extra.contains(&b))
                && s != "."
                && s != ".."
        };
        super::valid_name(&self.name)
            && ok(&self.version, &[])
            && (self.platform.is_empty() || ok(&self.platform, b"_-"))
    }
}

/// The most a `specs.4.8.gz` or a `.gemspec.rz` may inflate to.
const MAX_SPECS: u64 = 128 << 20;
const MAX_GEMSPEC: u64 = 16 << 20;

/// All of `reader`, or an error if it holds more than `limit` bytes.
fn inflate(reader: impl std::io::Read, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    reader.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(std::io::Error::other(format!(
            "it inflates to more than {limit} bytes"
        )));
    }
    Ok(data)
}

/// The triples of a gzipped `specs.4.8`, those that cannot name a file
/// left out.
pub fn parse_specs(gz: &[u8]) -> Result<Vec<Tuple>, String> {
    let data = inflate(flate2::read::GzDecoder::new(gz), MAX_SPECS)
        .map_err(|e| format!("not gzip: {e}"))?;
    let value = marshal::load(&data).map_err(|e| e.to_string())?;
    let items = value.as_array().ok_or("not an Array of name tuples")?;
    let mut tuples = Vec::new();
    for item in items {
        let parts = item.as_array().unwrap_or_default();
        let name = parts.first().and_then(Value::as_str);
        let version = parts.get(1).and_then(version_string);
        let (Some(name), Some(version)) = (name, version) else {
            continue;
        };
        let platform = parts.get(2).and_then(Value::as_str).unwrap_or("ruby");
        let tuple = Tuple {
            name: name.to_owned(),
            version,
            platform: platform.to_owned(),
        };
        if tuple.safe() {
            tuples.push(tuple);
        }
    }
    Ok(tuples)
}

/// The `/info` line of one build from its deflated `.gemspec.rz`.
pub fn info_line(tuple: &Tuple, rz: &[u8]) -> Result<String, String> {
    let data = inflate(flate2::read::ZlibDecoder::new(rz), MAX_GEMSPEC)
        .map_err(|e| format!("not deflated: {e}"))?;
    let spec = match marshal::load(&data).map_err(|e| e.to_string())? {
        Value::UserDump(class, dumped) if &*class == "Gem::Specification" => {
            marshal::load(&dumped).map_err(|e| e.to_string())?
        }
        _ => return Err("not a Gem::Specification".to_owned()),
    };
    // `Gem::Specification._load`'s array.
    let fields = spec.as_array().ok_or("not a Gem::Specification")?;
    let field = |i: usize| fields.get(i).unwrap_or(&Value::Nil);
    let mut deps: Vec<(String, String)> = Vec::new();
    for dep in field(9).as_array().unwrap_or_default() {
        let runtime = match dep.ivar("type") {
            Some(Value::Symbol(t)) => &**t == "runtime",
            _ => true,
        };
        let Some(name) = dep.ivar("name").and_then(Value::as_str) else {
            continue;
        };
        if !runtime {
            continue;
        }
        let requirement = dep
            .ivar("requirement")
            .or_else(|| dep.ivar("version_requirements"))
            .map(requirement_string)
            .unwrap_or_else(|| ">= 0".to_owned());
        deps.push((name.to_owned(), requirement));
    }
    deps.sort();
    let mut reqs = Vec::new();
    for (key, i) in [("ruby", 6), ("rubygems", 7)] {
        let requirement = requirement_string(field(i));
        if requirement != ">= 0" {
            reqs.push(format!("{key}:{requirement}"));
        }
    }
    let deps: Vec<String> = deps.iter().map(|(n, r)| format!("{n}:{r}")).collect();
    Ok(format!(
        "{} {}|{}",
        tuple.index_version(),
        deps.join(","),
        reqs.join(",")
    ))
}

/// A `Gem::Version`'s string: its `marshal_dump`, `[version]`, or an old
/// dump's `@version`.
fn version_string(value: &Value) -> Option<String> {
    match value {
        Value::UserMarshal(_, inner) => inner.as_array()?.first()?.as_str().map(str::to_owned),
        Value::Object(..) => value.ivar("version")?.as_str().map(str::to_owned),
        Value::Str(_) => value.as_str().map(str::to_owned),
        _ => None,
    }
}

/// A `Gem::Requirement` as an `/info` line writes one: `op version` pairs
/// joined by `&`, sorted; none is `>= 0`.
fn requirement_string(value: &Value) -> String {
    let pairs = match value {
        Value::UserMarshal(_, inner) => inner.as_array().and_then(|a| a.first()),
        Value::Object(..) => value.ivar("requirements"),
        _ => None,
    };
    let mut parts: Vec<String> = pairs
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|pair| {
            let pair = pair.as_array()?;
            let op = pair.first()?.as_str()?;
            let version = version_string(pair.get(1)?)?;
            Some(format!("{op} {version}"))
        })
        .collect();
    if parts.is_empty() {
        return ">= 0".to_owned();
    }
    parts.sort();
    parts.join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(path: &str) -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/private_source/legacy")
                .join(path),
        )
        .unwrap()
    }

    #[test]
    fn reads_specs_and_gemspecs_gem_generate_index_writes() {
        let mut tuples = parse_specs(&fixture("specs.4.8.gz")).unwrap();
        tuples.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<String> = tuples.iter().map(Tuple::full_name).collect();
        assert_eq!(
            names,
            [
                "base64-0.3.0",
                "rack-3.2.8",
                "rack-session-2.1.2",
                "rackup-2.3.1"
            ]
        );
        assert!(
            parse_specs(&fixture("prerelease_specs.4.8.gz"))
                .unwrap()
                .is_empty()
        );
        let session = &tuples[2];
        let rz = fixture("quick/Marshal.4.8/rack-session-2.1.2.gemspec.rz");
        assert_eq!(
            info_line(session, &rz).unwrap(),
            "2.1.2 base64:>= 0.1.0,rack:>= 3.0.0|ruby:>= 2.5"
        );
        assert!(info_line(session, b"junk").is_err());
    }

    #[test]
    fn an_index_that_inflates_too_far_is_refused() {
        use std::io::Write as _;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&[0; 4096]).unwrap();
        let gz = gz.finish().unwrap();
        assert!(inflate(flate2::read::GzDecoder::new(&gz[..]), 4095).is_err());
        assert_eq!(
            inflate(flate2::read::GzDecoder::new(&gz[..]), 4096)
                .unwrap()
                .len(),
            4096
        );

        let mut rz = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        rz.write_all(&vec![0; MAX_GEMSPEC as usize + 1]).unwrap();
        let rz = rz.finish().unwrap();
        let tuple = Tuple {
            name: "rack".to_owned(),
            version: "3.2.8".to_owned(),
            platform: "ruby".to_owned(),
        };
        let err = info_line(&tuple, &rz).unwrap_err();
        assert!(err.contains("inflates to more than"), "{err}");
    }

    #[test]
    fn a_tuple_that_cannot_name_a_file_is_left_out() {
        let bad = Tuple {
            name: "rack".to_owned(),
            version: "../../x".to_owned(),
            platform: "ruby".to_owned(),
        };
        assert!(!bad.safe());
        let native = Tuple {
            name: "nokogiri".to_owned(),
            version: "1.18.0".to_owned(),
            platform: "x86_64-linux-gnu".to_owned(),
        };
        assert!(native.safe());
        assert_eq!(native.full_name(), "nokogiri-1.18.0-x86_64-linux-gnu");
        assert_eq!(native.index_version(), "1.18.0-x86_64-linux-gnu");
    }
}

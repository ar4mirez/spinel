use super::*;

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.append(*name, HeaderValue::from_str(value).unwrap());
    }
    map
}

#[test]
fn etags_lose_their_quotes_and_weakness() {
    let etag = |v: &str| etag_from_response(&headers(&[("etag", v)]));
    assert_eq!(etag("\"abc\"").as_deref(), Some("abc"));
    assert_eq!(etag("W/\"abc\"").as_deref(), Some("abc"));
    assert_eq!(etag("abc").as_deref(), Some("abc"));
    // An opening quote with no closing one: Bundler stores nothing.
    assert_eq!(etag("\"abc"), None);
    assert_eq!(etag_from_response(&HeaderMap::new()), None);
}

#[test]
fn digests_are_base64_sha256_in_any_of_bundlers_wrappings() {
    let b64 = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(b"hello"));
    let digest = |pairs: &[(&'static str, &str)]| parse_digest(&headers(pairs));
    let want = Some(b64.clone());
    assert_eq!(
        digest(&[("repr-digest", &format!("sha-256=:{b64}:"))]),
        want
    );
    // What rubygems.org sends.
    assert_eq!(
        digest(&[("repr-digest", &format!("sha-256=\"{b64}\""))]),
        want
    );
    assert_eq!(digest(&[("digest", &format!("SHA-256={b64}"))]), want);
    assert_eq!(
        digest(&[("repr-digest", &format!("sha-512=:AAAA:, sha-256=:{b64}:"))]),
        want
    );
    // Repr-Digest wins over Digest, as in Bundler.
    assert_eq!(
        digest(&[
            ("repr-digest", &format!("sha-256=:{b64}:")),
            ("digest", "sha-256=:AAAA:")
        ]),
        want
    );
    assert_eq!(digest(&[("repr-digest", "sha-512=:AAAA:")]), None);
    assert_eq!(digest(&[("repr-digest", &format!("sha-256=:{b64}"))]), None);
    assert_eq!(
        digest(&[("repr-digest", &format!("sha-256=\"{b64}"))]),
        None
    );
    assert_eq!(digest(&[]), None);
    assert!(matches(b"hello", &b64));
    assert!(!matches(b"hello!", &b64));
}

#[test]
fn info_checksums_take_the_last_line_after_the_header() {
    let versions = b"created_at: 2024-04-01T00:00:05Z\n---\n\
        rack 1.0.0,1.1.0 aaaa\n\
        rails 7.0.0 bbbb\n\
        rack 1.2.0 cccc\n\
        rack-test 2.0.0 dddd\n";
    let found = info_checksums(versions, &["rack", "rails", "missing"]);
    assert_eq!(found.get("rack").map(String::as_str), Some("cccc"));
    assert_eq!(found.get("rails").map(String::as_str), Some("bbbb"));
    assert_eq!(found.get("missing"), None);
    assert_eq!(found.len(), 2);
    // No header line: every line is an entry.
    let found = info_checksums(b"rack 1.0.0 eeee\n", &["rack"]);
    assert_eq!(found.get("rack").map(String::as_str), Some("eeee"));
}

#[test]
fn the_cache_is_named_for_the_host() {
    let home = Path::new("/home");
    let dir = |source: &str| Client::new(source, home).unwrap().directory().to_owned();
    assert_eq!(
        dir("https://rubygems.org"),
        Path::new("/home/index/rubygems.org")
    );
    assert_eq!(
        dir("https://rubygems.org/"),
        Path::new("/home/index/rubygems.org")
    );
    assert_eq!(
        dir("https://RubyGems.org:443/"),
        Path::new("/home/index/rubygems.org")
    );
    assert_eq!(
        dir("http://127.0.0.1:8080"),
        Path::new("/home/index/127.0.0.1:8080")
    );
    let private = dir("https://gems.example/private");
    let name = private.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("gems.example-") && name.len() == "gems.example-".len() + 16);
    assert_ne!(private, dir("https://gems.example/other"));
    assert!(Client::new("ftp://rubygems.org", home).is_err());
    assert!(Client::new("rubygems.org", home).is_err());
}

#[test]
fn info_files_follow_bundlers_layout() {
    let client = Client::new("https://rubygems.org", Path::new("/h")).unwrap();
    assert_eq!(
        client.info_path("rack-test_2"),
        Path::new("/h/index/rubygems.org/info/rack-test_2")
    );
    let special = client.info_path("Rack.Pro");
    assert_eq!(
        special.parent().unwrap(),
        Path::new("/h/index/rubygems.org/info-special-characters")
    );
    let name = special.file_name().unwrap().to_str().unwrap();
    assert_eq!(name, format!("Rack.Pro-{}", hex(&Md5::digest("Rack.Pro"))));
    assert_ne!(special, client.info_path("rack.pro"));
}

#[test]
fn names_outside_rubygems_charset_are_refused() {
    for good in ["rack", "rack-test", "net_http", "Rack.Pro", "a1"] {
        assert!(valid_name(good), "{good}");
    }
    for bad in ["", ".", "..", "../etc", "a/b", "a b", "ä"] {
        assert!(!valid_name(bad), "{bad}");
    }
}

#[test]
fn an_info_line_gives_its_version_s_checksum() {
    let info = b"---\n1.0.0 |checksum:AA11,ruby:>= 2.7\n1.0.0-x86_64-linux rack:>= 2.0&< 4|checksum:bb22\n2.0.0 rack:>= 2.0|ruby:>= 3.0\n";
    assert_eq!(
        version_checksum(info, "1.0.0"),
        Some(Some("aa11".to_owned()))
    );
    assert_eq!(
        version_checksum(info, "1.0.0-x86_64-linux"),
        Some(Some("bb22".to_owned()))
    );
    // Listed, but by a server that gives no checksum.
    assert_eq!(version_checksum(info, "2.0.0"), Some(None));
    assert_eq!(version_checksum(info, "3.0.0"), None);
    // A prefix of a version is not the version.
    assert_eq!(version_checksum(info, "1.0"), None);
}

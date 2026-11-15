use super::*;

fn parts(path: &str) -> Vec<Vec<u8>> {
    path.split('/').map(|p| p.as_bytes().to_vec()).collect()
}

#[test]
fn gems_are_named_as_rubygems_names_them() {
    let gem = GemId::parse("rack@3.1.8").unwrap();
    assert_eq!(gem.platform, "ruby");
    assert_eq!(gem.full_name(), "rack-3.1.8");
    assert_eq!(gem.index_version(), "3.1.8");
    let gem = GemId::parse("nokogiri@1.18.1-x86_64-linux-gnu").unwrap();
    assert_eq!(gem.version, "1.18.1");
    assert_eq!(gem.platform, "x86_64-linux-gnu");
    assert_eq!(gem.full_name(), "nokogiri-1.18.1-x86_64-linux-gnu");
    assert_eq!(gem.index_version(), "1.18.1-x86_64-linux-gnu");
    // Nothing that could leave the store's directory or the source's path.
    for bad in [
        "rack",
        "../x@1.0",
        "rack@../1",
        "rack@1/0",
        "rack@.1",
        "rack@1.0-x/y",
        "rack@",
        "@1.0",
    ] {
        assert!(GemId::parse(bad).is_err(), "{bad} parsed");
    }
}

#[test]
fn an_entry_name_must_stay_inside_the_gem() {
    let ok = |p: &str| within(&[], p.as_bytes()).map(|v| String::from_utf8(v.join(&b'/')).unwrap());
    assert_eq!(ok("lib/a.rb").as_deref(), Some("lib/a.rb"));
    assert_eq!(ok("./lib//a.rb").as_deref(), Some("lib/a.rb"));
    assert_eq!(ok("lib/../a.rb").as_deref(), Some("a.rb"));
    assert_eq!(ok("../a.rb"), None);
    assert_eq!(ok("lib/../../a.rb"), None);
    assert_eq!(ok("/etc/passwd"), None);
    // The directory itself, which RubyGems refuses too.
    assert_eq!(ok("."), None);
    assert_eq!(ok("lib/.."), None);
}

#[test]
fn a_symlink_target_is_read_from_the_link_s_directory() {
    let base = parts("lib/beta");
    assert!(within(&base, b"version.rb").is_some());
    assert!(within(&base, b"../../README").is_some());
    assert!(within(&base, b"../../../etc/passwd").is_none());
    assert!(within(&base, b"../..").is_none());
    assert!(within(&base, b"/etc/passwd").is_none());
}

#[test]
fn checksums_yaml_is_read_as_rubygems_writes_it() {
    let yaml =
        "---\nSHA256:\n  metadata.gz: AB12\n  data.tar.gz: cd34\nSHA512:\n  metadata.gz: '99'\n";
    assert_eq!(
        parse_checksums(yaml),
        vec![
            (
                "SHA256".to_owned(),
                vec![
                    ("data.tar.gz".to_owned(), "cd34".to_owned()),
                    ("metadata.gz".to_owned(), "AB12".to_owned()),
                ]
            ),
            (
                "SHA512".to_owned(),
                vec![("metadata.gz".to_owned(), "99".to_owned())]
            ),
        ]
    );
}

/// A data.tar.gz-shaped tar (uncompressed) of symlinks, files, and
/// directories (a name ending in `/`).
fn tar_of(entries: &[(&str, Option<&str>)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, link) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        if name.ends_with('/') {
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_path(name).unwrap();
            header.set_cksum();
            builder.append(&header, &[][..]).unwrap();
            continue;
        }
        match link {
            Some(target) => {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_link_name(target).unwrap();
                header.set_path(name).unwrap();
                header.set_cksum();
                builder.append(&header, &[][..]).unwrap();
            }
            None => {
                header.set_size(2);
                header.set_path(name).unwrap();
                header.set_cksum();
                builder.append(&header, &b"x\n"[..]).unwrap();
            }
        }
    }
    builder.into_inner().unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "spinel-gem-store-unit-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("gem")).unwrap();
    dir
}

#[test]
fn a_symlink_inside_by_its_spelling_but_outside_through_another_is_refused() {
    let dir = scratch("through");
    let data = tar_of(&[
        ("a/b/c/up", Some("../..")),
        ("a/b/c/up/x", Some("../../..")),
    ]);
    let err = extract("evil-1.0", &data[..], &dir.join("gem")).unwrap_err();
    assert!(
        matches!(&err, Error::Symlink { entry, .. } if entry == "a/b/c/up/x"),
        "{err}"
    );
    // The first one, which leads inside, is fine on its own.
    let dir = scratch("inside");
    let data = tar_of(&[("a/b/c/up", Some("../.."))]);
    assert!(extract("ok-1.0", &data[..], &dir.join("gem")).is_ok());
}

#[test]
fn a_symlink_may_not_replace_a_directory_another_symlink_leads_through() {
    let dir = scratch("redirect");
    fs::create_dir_all(dir.join("n")).unwrap();
    fs::write(dir.join("n/victim"), "kept\n").unwrap();
    let data = tar_of(&[
        ("p/q/", None),
        ("n/", None),
        ("x/", None),
        ("k/l", Some("../p/q/../../n")),
        ("p/q", Some("../x")),
        ("k/l/victim", Some("y")),
    ]);
    let err = extract("evil-1.0", &data[..], &dir.join("gem")).unwrap_err();
    assert!(
        matches!(&err, Error::Symlink { entry, .. } if entry == "p/q"),
        "{err}"
    );
    assert!(dir.join("gem/p/q").is_dir());
    let victim = fs::symlink_metadata(dir.join("n/victim")).unwrap();
    assert!(victim.is_file());
    assert_eq!(fs::read_to_string(dir.join("n/victim")).unwrap(), "kept\n");
    let mut outside: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    outside.sort();
    assert_eq!(outside, ["gem", "n"]);
    assert_eq!(fs::read_dir(dir.join("n")).unwrap().count(), 1);
}

#[test]
fn a_symlink_may_not_replace_an_earlier_symlink() {
    let dir = scratch("twice");
    let data = tar_of(&[
        ("lib/a.rb", None),
        ("lib/b.rb", None),
        ("lib/link.rb", Some("a.rb")),
        ("lib/link.rb", Some("b.rb")),
    ]);
    let err = extract("twice-1.0", &data[..], &dir.join("gem")).unwrap_err();
    assert!(
        matches!(&err, Error::Symlink { entry, target, .. } if entry == "lib/link.rb" && target == "b.rb"),
        "{err}"
    );
}

#[test]
fn a_dangling_symlink_is_left_out_with_rubygems_warning() {
    let dir = scratch("dangling");
    let data = tar_of(&[("lib/a.rb", None), ("lib/gone.rb", Some("missing.rb"))]);
    let warnings = extract("d-1.0", &data[..], &dir.join("gem")).unwrap();
    assert_eq!(
        warnings,
        [
            "d-1.0 ships with a dangling symlink named lib/gone.rb pointing to missing missing.rb file. Ignoring"
        ]
    );
    assert!(fs::symlink_metadata(dir.join("gem/lib/gone.rb")).is_err());
    assert!(dir.join("gem/lib/a.rb").is_file());
}

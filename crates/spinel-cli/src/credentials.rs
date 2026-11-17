//! Credentials for a gem source (#89, docs/package-manager.md, "Index
//! client"), taken the way Bundler 4 takes them.
//!
//! A source's own URL may carry them (`https://user:secret@gems.example`).
//! When it carries none, `Bundler::Settings#credentials_for` looks them up by
//! the source's URL and then by its host, `BUNDLE_<KEY>` in the project's
//! `.bundle/config`, then the environment, then the global config, the first
//! found winning: `bundle config set --global gems.example user:secret`
//! writes `BUNDLE_GEMS__EXAMPLE`, as does exporting it. The user and password
//! are percent-decoded (`CGI.unescape`) and sent as HTTP Basic credentials,
//! to that source's host only.
//!
//! Nothing here is ever printed: [`Auth`]'s `Debug` hides it, and the URL a
//! client keeps has had them taken out.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use reqwest::Url;

/// A user and password for HTTP Basic authentication, decoded.
#[derive(Clone, PartialEq, Eq)]
pub struct Auth {
    pub user: String,
    pub password: Option<String>,
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Auth([REDACTED])")
    }
}

impl Auth {
    /// `user:password` as a setting holds it, or a bare token as the user.
    /// Percent-escapes are decoded, as Bundler's downloader decodes the
    /// userinfo it puts them in.
    pub fn parse(value: &str) -> Option<Auth> {
        if value.is_empty() {
            return None;
        }
        let (user, password) = match value.split_once(':') {
            Some((user, password)) => (user, Some(password)),
            None => (value, None),
        };
        Some(Auth {
            user: unescape(user),
            password: password.map(unescape),
        })
    }
}

/// The URL's own credentials, taken out of it.
pub fn take_userinfo(url: &mut Url) -> Option<Auth> {
    if url.username().is_empty() && url.password().is_none() {
        return None;
    }
    let auth = Auth {
        user: unescape(url.username()),
        password: url.password().map(unescape),
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    Some(auth)
}

/// `CGI.unescape`: `%XX` decoded, `+` a space.
fn unescape(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit() =>
            {
                let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The Bundler settings credentials are read from: the project's
/// `.bundle/config`, the `BUNDLE_*` environment and the global config, in
/// that order.
#[derive(Default, Clone)]
pub struct Settings {
    local: HashMap<String, String>,
    env: HashMap<String, String>,
    global: HashMap<String, String>,
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Settings([REDACTED])")
    }
}

impl Settings {
    /// The settings for a project at `root` (`None`: no project, so no local
    /// config), read from this process's environment.
    pub fn load(root: Option<&Path>) -> Settings {
        let vars: HashMap<String, String> = std::env::vars().collect();
        Settings::from_env(root, &vars)
    }

    /// [`Settings::load`] with the environment given.
    pub fn from_env(root: Option<&Path>, vars: &HashMap<String, String>) -> Settings {
        let set = |k: &str| vars.get(k).filter(|v| !v.is_empty());
        let ignore = vars.contains_key("BUNDLE_IGNORE_CONFIG");
        let local = match root {
            Some(root) if !ignore => {
                // `Bundler.app_config_path`: `BUNDLE_APP_CONFIG`, relative to
                // the project, else `.bundle`.
                let dir =
                    set("BUNDLE_APP_CONFIG").map_or_else(|| root.join(".bundle"), |d| root.join(d));
                load_config(&dir.join("config"))
            }
            _ => HashMap::new(),
        };
        let global = if ignore {
            HashMap::new()
        } else {
            let path: Option<PathBuf> = set("BUNDLE_CONFIG")
                .or_else(|| set("BUNDLE_USER_CONFIG"))
                .map(PathBuf::from)
                .or_else(|| set("BUNDLE_USER_HOME").map(|h| Path::new(h).join("config")))
                .or_else(|| set("HOME").map(|h| Path::new(h).join(".bundle/config")));
            path.map(|p| load_config(&p)).unwrap_or_default()
        };
        let env = vars
            .iter()
            .filter(|(k, _)| k.starts_with("BUNDLE_") && k.as_str() != "BUNDLE_")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Settings { local, env, global }
    }

    /// `Settings#[]`: the first level that has `key`.
    fn get(&self, key: &str) -> Option<&str> {
        let key = key_for(key);
        [&self.local, &self.env, &self.global]
            .into_iter()
            .find_map(|level| level.get(&key))
            .map(String::as_str)
    }

    /// `credentials_for`: by the source's URL (no credentials in it), then
    /// by its host.
    pub fn credentials_for(&self, url: &Url) -> Option<Auth> {
        self.get(url.as_str())
            .or_else(|| url.host_str().and_then(|host| self.get(host)))
            .and_then(Auth::parse)
    }
}

/// `Settings.key_for`: `BUNDLE_` and the key upcased, `.` as `__` and `-` as
/// `___`; a URL first given a trailing `/`.
pub fn key_for(key: &str) -> String {
    let mut key = key.to_owned();
    if key.starts_with("http") && !key.ends_with('/') {
        key.push('/');
    }
    let key = key
        .replace('.', "__")
        .replace('-', "___")
        .to_ascii_uppercase();
    format!("BUNDLE_{key}")
}

/// A config file as `Settings#load_config` reads it: `KEY: value` lines, a
/// URL key given a trailing `/` and its `.` written `__`, `-` written `___`.
/// A file that cannot be read is empty.
fn load_config(path: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let mut config = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        let mut key = key.trim().to_owned();
        if key.starts_with('#') || key.starts_with("---") || key.is_empty() {
            continue;
        }
        let lower = key.to_ascii_lowercase();
        if (lower.contains("http:") || lower.contains("https:")) && !key.ends_with('/') {
            key.push('/');
        }
        let key = key.replace('.', "__").replace('-', "___");
        config.insert(key, yaml_scalar(value.trim()));
    }
    config
}

/// A scalar as `Gem::YAMLSerializer` writes one: double-quoted with
/// backslash escapes, single-quoted, or plain.
fn yaml_scalar(value: &str) -> String {
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match (c, c == '\\') {
                (_, true) => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => {}
                },
                (c, false) => out.push(c),
            }
        }
        out
    } else if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        inner.replace("''", "'")
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_bundlers() {
        assert_eq!(key_for("127.0.0.1"), "BUNDLE_127__0__0__1");
        assert_eq!(key_for("gems.my-host.io"), "BUNDLE_GEMS__MY___HOST__IO");
        assert_eq!(
            key_for("https://gems.example.com"),
            "BUNDLE_HTTPS://GEMS__EXAMPLE__COM/"
        );
    }

    #[test]
    fn userinfo_is_taken_out_and_decoded() {
        let mut url = Url::parse("https://us%40er:p%3Ass+w@gems.example/private/").unwrap();
        let auth = take_userinfo(&mut url).unwrap();
        assert_eq!(auth.user, "us@er");
        assert_eq!(auth.password.as_deref(), Some("p:ss w"));
        assert_eq!(url.as_str(), "https://gems.example/private/");
        let mut token = Url::parse("https://TOKEN@gems.example/").unwrap();
        let auth = take_userinfo(&mut token).unwrap();
        assert_eq!(
            (auth.user.as_str(), auth.password.as_deref()),
            ("TOKEN", None)
        );
        assert_eq!(format!("{auth:?}"), "Auth([REDACTED])");
    }

    #[test]
    fn local_then_env_then_global_by_url_then_host() {
        let dir = std::env::temp_dir().join(format!("spinel-credentials-{}", std::process::id()));
        let project = dir.join("project");
        std::fs::create_dir_all(project.join(".bundle")).unwrap();
        std::fs::create_dir_all(dir.join("user")).unwrap();
        std::fs::write(
            project.join(".bundle/config"),
            "---\nBUNDLE_PATH: \".spinel/gem_home\"\nBUNDLE_LOCAL__EXAMPLE: \"local:one\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("user/config"),
            "---\nBUNDLE_HTTPS://GLOBAL__EXAMPLE/: \"url:two\"\nBUNDLE_LOCAL__EXAMPLE: \"global:x\"\nBUNDLE_ENV__EXAMPLE: \"global:y\"\n",
        )
        .unwrap();
        let vars: HashMap<String, String> = [
            (
                "BUNDLE_USER_HOME",
                dir.join("user").to_string_lossy().into_owned(),
            ),
            ("BUNDLE_ENV__EXAMPLE", "env:three".to_owned()),
            ("BUNDLE_LOCAL__EXAMPLE", "env:x".to_owned()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let settings = Settings::from_env(Some(&project), &vars);
        let auth = |url: &str| settings.credentials_for(&Url::parse(url).unwrap());
        assert_eq!(auth("https://local.example/").unwrap().user, "local");
        assert_eq!(auth("https://env.example/").unwrap().user, "env");
        assert_eq!(auth("https://global.example/").unwrap().user, "url");
        assert!(auth("https://other.example/").is_none());
        let mut ignored = vars.clone();
        ignored.insert("BUNDLE_IGNORE_CONFIG".to_owned(), "1".to_owned());
        let settings = Settings::from_env(Some(&project), &ignored);
        assert_eq!(
            settings
                .credentials_for(&Url::parse("https://local.example/").unwrap())
                .unwrap()
                .user,
            "env"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

//! The compact index client (#77, docs/package-manager.md, "Index client").
//!
//! rubygems.org's compact index is the API Bundler resolves against: `/versions`
//! (every gem and version, append-only), `/info/<gem>` (one line per version
//! with its dependencies and checksum) and `/names`. Each is cached under
//! `<spinel home>/index/<host>/` and brought up to date the way Bundler's
//! `CompactIndexClient::Updater` does it:
//!
//! - With a non-empty cached copy, ask for the bytes from one before its end
//!   (`Range: bytes=<size - 1>-`, so the range is never empty and never a 416),
//!   with `If-None-Match` carrying the stored ETag. A 304 leaves the copy as it
//!   is. A 206 is appended without its first byte, which the copy already has,
//!   and kept only when the whole result matches the `Repr-Digest` (or
//!   `Digest`) SHA-256 the server sent: appending is too error prone without
//!   one, so a 206 with no digest is not appended at all. A 200 (a server that
//!   ignored the range) replaces the copy.
//! - When that does not settle it (no cached copy, no digest, a mismatch, a
//!   cache longer than the file now is), fetch the whole file again, with the
//!   ETag but no range. A whole file whose digest does not match is an error,
//!   never written.
//!
//! An `/info/<gem>` file is fetched only when its MD5 differs from the one the
//! cached `/versions` lists last for that gem, as Bundler skips it, so a warm
//! cache costs one request for any number of gems. At most [`MAX_CONCURRENT`]
//! requests are in flight at once.
//!
//! Every file is written to a temporary name beside it and renamed into
//! place, so a reader (another `spinel` running now) sees the old copy or the
//! new one, never half of one.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use md5::Md5;
use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{StatusCode, Url};
use sha2::{Digest as _, Sha256};
use tokio::sync::Semaphore;

/// The most requests one client has in flight at once.
pub const MAX_CONCURRENT: usize = 32;

/// Where a gem comes from when the Gemfile names no source.
pub const DEFAULT_SOURCE: &str = "https://rubygems.org";

/// What an update did to one cached file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The whole file was fetched and written: this many bytes.
    Fetched(u64),
    /// The cached copy was extended by this many bytes, verified against the
    /// server's digest of the whole file.
    Appended(u64),
    /// The server answered 304: the cached copy is current.
    NotModified,
    /// No request: the cached `/info` file has the checksum `/versions` lists.
    Current,
    /// No request: `/versions` does not list the gem.
    Unknown,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Fetched(n) => write!(f, "fetched {n} bytes"),
            Outcome::Appended(n) => write!(f, "appended {n} bytes"),
            Outcome::NotModified => f.write_str("not modified"),
            Outcome::Current => f.write_str("current"),
            Outcome::Unknown => f.write_str("not in the index"),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    /// The source is not an `http`/`https` URL with a host.
    Source(String),
    /// A gem name with a character rubygems.org does not allow in one, which
    /// could otherwise name a path outside the cache.
    Name(String),
    Http(Url, reqwest::Error),
    Status(Url, StatusCode),
    /// A whole file that does not match the digest the server sent with it.
    Digest(Url),
    Io(PathBuf, io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Source(s) => write!(f, "{s}: not an http or https source URL"),
            Error::Name(n) => write!(f, "{n:?}: not a gem name"),
            Error::Http(url, e) => write!(f, "{}: {e}", redacted(url)),
            Error::Status(url, code) => write!(f, "{}: HTTP {code}", redacted(url)),
            Error::Digest(url) => write!(
                f,
                "{}: the response does not match the digest the server sent with it",
                redacted(url)
            ),
            Error::Io(path, e) => write!(f, "{}: {e}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

/// A URL with any password left out, for messages.
fn redacted(url: &Url) -> Url {
    let mut url = url.clone();
    let _ = url.set_password(None);
    url
}

/// One source's compact index and its cache.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    /// The source, ending in `/`, so a join appends to its path.
    base: Url,
    dir: PathBuf,
    limit: Arc<Semaphore>,
}

impl Client {
    /// A client for `source`, caching under `home`'s `index/<host>/`.
    pub fn new(source: &str, home: &Path) -> Result<Client, Error> {
        let mut base = Url::parse(source).map_err(|_| Error::Source(source.to_owned()))?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(Error::Source(source.to_owned()));
        }
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        base.set_query(None);
        base.set_fragment(None);
        let http = reqwest::Client::builder()
            .user_agent(concat!("spinel/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Http(base.clone(), e))?;
        let dir = home.join("index").join(cache_name(&base));
        Ok(Client {
            http,
            base,
            dir,
            limit: Arc::new(Semaphore::new(MAX_CONCURRENT)),
        })
    }

    /// The cache directory, `<home>/index/<host>/`.
    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// Bring the cached `/versions` up to date.
    pub async fn versions(&self) -> Result<Outcome, Error> {
        self.update(
            "versions",
            &self.dir.join("versions"),
            &self.dir.join("versions.etag"),
        )
        .await
    }

    /// Bring the cached `/names` up to date.
    pub async fn names(&self) -> Result<Outcome, Error> {
        self.update(
            "names",
            &self.dir.join("names"),
            &self.dir.join("names.etag"),
        )
        .await
    }

    /// Bring the cached `/info/<gem>` of each gem up to date, as many at once
    /// as the concurrency limit allows. A gem whose cached file already has
    /// the checksum the cached `/versions` lists costs no request, and one it
    /// does not list is not fetched. Results come back in `gems`' order, a
    /// gem named twice once, as its first: two updates of one file at once
    /// would race for its temporary file.
    pub async fn info(
        &self,
        gems: &[String],
    ) -> Result<Vec<(String, Result<Outcome, Error>)>, Error> {
        let mut seen = std::collections::HashSet::new();
        let gems: Vec<String> = gems
            .iter()
            .filter(|g| seen.insert(g.as_str()))
            .cloned()
            .collect();
        let versions = self.dir.join("versions");
        let wanted: Vec<&str> = gems.iter().map(String::as_str).collect();
        let checksums = match std::fs::read(&versions) {
            Ok(data) => info_checksums(&data, &wanted),
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(Error::Io(versions, e)),
        };
        let mut results: Vec<Option<Result<Outcome, Error>>> = Vec::new();
        let mut tasks = tokio::task::JoinSet::new();
        for (i, gem) in gems.iter().enumerate() {
            results.push(None);
            if !valid_name(gem) {
                results[i] = Some(Err(Error::Name(gem.clone())));
                continue;
            }
            let Some(remote) = checksums.get(gem.as_str()) else {
                results[i] = Some(Ok(Outcome::Unknown));
                continue;
            };
            let local = self.info_path(gem);
            if file_md5(&local).is_some_and(|md5| md5 == *remote) {
                results[i] = Some(Ok(Outcome::Current));
                continue;
            }
            let client = self.clone();
            let etag = self.info_etag_path(gem);
            let remote_path = format!("info/{gem}");
            tasks.spawn(async move { (i, client.update(&remote_path, &local, &etag).await) });
        }
        while let Some(joined) = tasks.join_next().await {
            let (i, result) = joined.expect("an index task panicked");
            results[i] = Some(result);
        }
        Ok(gems
            .iter()
            .cloned()
            .zip(
                results
                    .into_iter()
                    .map(|r| r.expect("every gem has a result")),
            )
            .collect())
    }

    /// Where `/info/<gem>` is cached. Bundler's layout: a name of only
    /// lowercase letters, digits, `_` and `-` is its own file name; any other
    /// goes under `info-special-characters/` with its MD5 after it, so two
    /// names that differ only in case stay two files on a case-insensitive
    /// file system.
    pub fn info_path(&self, gem: &str) -> PathBuf {
        if gem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            self.dir.join("info").join(gem)
        } else {
            self.dir
                .join("info-special-characters")
                .join(format!("{gem}-{}", hex(&Md5::digest(gem))))
        }
    }

    fn info_etag_path(&self, gem: &str) -> PathBuf {
        self.dir
            .join("info-etags")
            .join(format!("{gem}-{}", hex(&Md5::digest(gem))))
    }

    /// Bundler's `Updater#update`: append to the cached copy if that can be
    /// verified, else fetch it whole. One permit covers both requests, so the
    /// limit counts requests in flight.
    async fn update(&self, remote: &str, local: &Path, etag_path: &Path) -> Result<Outcome, Error> {
        let _permit = self
            .limit
            .acquire()
            .await
            .expect("the semaphore is never closed");
        let url = self
            .base
            .join(remote)
            .map_err(|_| Error::Source(self.base.to_string()))?;
        if let Some(outcome) = self.append(&url, local, etag_path).await? {
            return Ok(outcome);
        }
        self.replace(&url, local, etag_path).await
    }

    /// The incremental request. `None` when it settles nothing and the whole
    /// file has to be fetched.
    async fn append(
        &self,
        url: &Url,
        local: &Path,
        etag_path: &Path,
    ) -> Result<Option<Outcome>, Error> {
        let size = match std::fs::metadata(local) {
            Ok(meta) if meta.is_file() && meta.len() > 0 => meta.len(),
            _ => return Ok(None),
        };
        let etag = read_etag(etag_path);
        let response = self.get(url, etag.as_deref(), Some(size - 1)).await?;
        match response.status() {
            StatusCode::NOT_MODIFIED => Ok(Some(Outcome::NotModified)),
            StatusCode::PARTIAL_CONTENT => {
                let digest = parse_digest(response.headers());
                let new_etag = etag_from_response(response.headers());
                let body = response
                    .bytes()
                    .await
                    .map_err(|e| Error::Http(url.clone(), e))?;
                let (Some(digest), Some(tail)) = (digest, body.get(1..)) else {
                    return Ok(None);
                };
                let mut data = std::fs::read(local).map_err(|e| Error::Io(local.to_owned(), e))?;
                data.extend_from_slice(tail);
                if !matches(&data, &digest) {
                    return Ok(None);
                }
                write_atomic(local, &data)?;
                write_etag(etag_path, new_etag.as_deref())?;
                Ok(Some(Outcome::Appended(tail.len() as u64)))
            }
            // Asked for a range starting inside the cached copy, a server that
            // answers 416 has a shorter file than the cache. Bundler's
            // downloader asks again without the range; so does this.
            StatusCode::RANGE_NOT_SATISFIABLE => Ok(None),
            status if status.is_success() => self
                .write_whole(url, response, local, etag_path)
                .await
                .map(Some),
            status => Err(Error::Status(url.clone(), status)),
        }
    }

    /// The full request, with the ETag but no range.
    async fn replace(&self, url: &Url, local: &Path, etag_path: &Path) -> Result<Outcome, Error> {
        // Bundler sends the ETag even with no cached copy; a 304 would then
        // leave nothing, so without one there is no ETag to send.
        let etag = local.is_file().then(|| read_etag(etag_path)).flatten();
        let response = self.get(url, etag.as_deref(), None).await?;
        match response.status() {
            StatusCode::NOT_MODIFIED if etag.is_some() => Ok(Outcome::NotModified),
            StatusCode::OK => self.write_whole(url, response, local, etag_path).await,
            status => Err(Error::Status(url.clone(), status)),
        }
    }

    async fn write_whole(
        &self,
        url: &Url,
        response: reqwest::Response,
        local: &Path,
        etag_path: &Path,
    ) -> Result<Outcome, Error> {
        let digest = parse_digest(response.headers());
        let new_etag = etag_from_response(response.headers());
        let body = response
            .bytes()
            .await
            .map_err(|e| Error::Http(url.clone(), e))?;
        if digest.is_some_and(|d| !matches(&body, &d)) {
            return Err(Error::Digest(url.clone()));
        }
        write_atomic(local, &body)?;
        write_etag(etag_path, new_etag.as_deref())?;
        Ok(Outcome::Fetched(body.len() as u64))
    }

    async fn get(
        &self,
        url: &Url,
        etag: Option<&str>,
        from: Option<u64>,
    ) -> Result<reqwest::Response, Error> {
        let mut request = self.http.get(url.clone());
        if let Some(etag) = etag {
            request = request.header(header::IF_NONE_MATCH, format!("\"{etag}\""));
        }
        if let Some(from) = from {
            // A range is of the bytes as sent: ask for them unencoded, or a
            // compressed tail would be appended to an uncompressed file.
            request = request
                .header(header::RANGE, format!("bytes={from}-"))
                .header(
                    header::ACCEPT_ENCODING,
                    HeaderValue::from_static("identity"),
                );
        }
        request
            .send()
            .await
            .map_err(|e| Error::Http(url.clone(), e))
    }
}

/// The cache directory's name for a source: its host, then `:<port>` for a
/// port that is not the scheme's own, then `-<digest>` of its path when it is
/// not the root, so two sources on one host never share a cache.
fn cache_name(base: &Url) -> String {
    let mut name = base.host_str().unwrap_or_default().to_ascii_lowercase();
    if let Some(port) = base.port() {
        name.push_str(&format!(":{port}"));
    }
    if base.path() != "/" {
        name.push('-');
        name.push_str(&hex(&Sha256::digest(base.path()))[..16]);
    }
    name
}

/// rubygems.org's rule for a gem name: letters, digits, `.`, `-` and `_`.
fn valid_name(gem: &str) -> bool {
    !gem.is_empty()
        && gem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        && gem != "."
        && gem != ".."
}

/// The info checksum `/versions` lists for each of `gems`: the last field of
/// the last line naming it, after the `---` that ends the header. Bundler's
/// `Parser#info_checksum`.
pub fn info_checksums(versions: &[u8], gems: &[&str]) -> HashMap<String, String> {
    let lines: Vec<&[u8]> = versions.split(|&b| b == b'\n').collect();
    let body = match lines.iter().position(|line| *line == b"---") {
        Some(header) => &lines[header + 1..],
        None => &lines[..],
    };
    let mut found = HashMap::new();
    for line in body {
        let Some(name_end) = line.iter().position(|&b| b == b' ') else {
            continue;
        };
        let name = &line[..name_end];
        if !gems.iter().any(|g| g.as_bytes() == name) {
            continue;
        }
        let Some(gap) = line[name_end + 1..].iter().position(|&b| b == b' ') else {
            continue;
        };
        let checksum = &line[name_end + 1 + gap + 1..];
        found.insert(
            String::from_utf8_lossy(name).into_owned(),
            String::from_utf8_lossy(checksum).into_owned(),
        );
    }
    found
}

/// The base64 SHA-256 a response carries, from `Repr-Digest` or else
/// `Digest`. Bundler's `parse_digests`: an RFC 8941 byte sequence
/// (`sha-256=:<base64>:`), a quoted string (`sha-256="<base64>"`, which is
/// what rubygems.org sends) or bare base64; a wrapper opened and not closed
/// is no digest. Other algorithms are ignored.
fn parse_digest(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get("repr-digest")
        .or_else(|| headers.get("digest"))?
        .to_str()
        .ok()?;
    value.split(',').find_map(|param| {
        let (algorithm, value) = param.split_once('=')?;
        if !algorithm.trim().eq_ignore_ascii_case("sha-256") {
            return None;
        }
        let mut value = value.trim();
        for wrapper in [':', '"'] {
            if let Some(inner) = value.strip_prefix(wrapper) {
                value = inner.strip_suffix(wrapper)?;
            }
        }
        Some(value.to_owned())
    })
}

/// Whether `data` has the digest the server sent, compared as Bundler
/// compares them, base64 to base64.
fn matches(data: &[u8], digest: &str) -> bool {
    base64::engine::general_purpose::STANDARD.encode(Sha256::digest(data)) == digest
}

/// The ETag to store from a response: weak or strong, without its quotes.
/// Bundler's `etag_from_response`: an opening quote with no closing one
/// stores nothing.
fn etag_from_response(headers: &HeaderMap) -> Option<String> {
    let etag = headers.get(header::ETAG)?.to_str().ok()?;
    let etag = etag.strip_prefix("W/").unwrap_or(etag);
    match etag.strip_prefix('"') {
        Some(inner) => inner.strip_suffix('"').map(str::to_owned),
        None => Some(etag.to_owned()),
    }
}

fn read_etag(path: &Path) -> Option<String> {
    let etag = std::fs::read_to_string(path).ok()?;
    let etag = etag.strip_suffix('\n').unwrap_or(&etag);
    let etag = etag.strip_suffix('\r').unwrap_or(etag);
    (!etag.is_empty()).then(|| etag.to_owned())
}

/// Store the new ETag, or remove the old one when the response had none, so
/// a stale ETag never vouches for a file it was not sent with.
fn write_etag(path: &Path, etag: Option<&str>) -> Result<(), Error> {
    match etag {
        Some(etag) => write_atomic(path, etag.as_bytes()),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(Error::Io(path.to_owned(), e)),
            _ => Ok(()),
        },
    }
}

/// Write `data` to a temporary file beside `path` and rename it into place.
fn write_atomic(path: &Path, data: &[u8]) -> Result<(), Error> {
    let io_error = |e| Error::Io(path.to_owned(), e);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, data).map_err(io_error)?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        io_error(e)
    })
}

fn file_md5(path: &Path) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    Some(hex(&Md5::digest(&data)))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests;

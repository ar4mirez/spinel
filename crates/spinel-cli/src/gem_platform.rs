//! `Gem::Platform` ported from RubyGems (#76, docs/package-manager.md,
//! "Resolver"): reading a platform string into cpu, os and version, the
//! `===` and `=~` matches, `match_platforms?` (the check behind `Gem.platforms`
//! and Bundler's `installable_on_platform?`), `generic`, and the specificity
//! score Bundler sorts candidates by.
//!
//! RubyGems' `lib/rubygems/platform.rb` is the source of truth, and this
//! follows it step for step, regular expressions included: they are unanchored
//! unless they say otherwise, `^` and `$` are Ruby's line anchors, `\d` and
//! `\w` are ASCII, and the first `when` that matches wins. Where its behavior
//! looks odd it is kept, with a note:
//!
//! - The os is found by searching, not by reading a field: anything with
//!   `darwin` in it is darwin, so `arm64-darwin-spinel` is `arm64-darwin`
//!   with no version (the suffix is dropped) and matches every
//!   `arm64-darwin-NN`. Linux and mingw keep a word after them as the version,
//!   so `x86_64-linux-spinel` is `["x86_64", "linux", "spinel"]` and matches
//!   no runtime. The tests pin which `<cpu>-<os>-spinel` spellings survive.
//! - A missing version is a wildcard on both sides, except on linux, where
//!   the version is the libc: a runtime with none is glibc, and a gem with
//!   none also fits a musl runtime, but a musl gem does not fit a glibc one.
//!   So `===` is not symmetric; the receiver is the gem's platform and the
//!   argument the runtime's.
//! - A missing cpu is a wildcard, as is `universal`; `universal` on one side
//!   matches any mingw on the other whatever the os version.
//! - `i386`..`i986` (any `i<digit>86` anywhere in the cpu) is `x86`.
//! - A lone word is an os with no cpu (`java`, `mswin32`), and `to_s` of a
//!   platform with no cpu runs the os and version together: `darwin0`.
//! - Words no rule knows become the os `unknown`, so all of them are equal.
//!
//! Not ported: `sort_and_filter_best_platform_match`, which compares whole
//! specifications (their dependencies and required versions) and belongs with
//! the resolver that has them; and TruffleRuby's per-gem `match_gem?`.

use std::borrow::Cow;
use std::fmt;

/// A platform RubyGems can match: cpu, os and version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Platform {
    cpu: Option<String>,
    os: String,
    version: Option<String>,
}

/// What `Gem::Platform.new` returns: the string `"ruby"` for pure-Ruby gems
/// (and for `nil` and `""`), otherwise a [`Platform`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GemPlatform {
    Ruby,
    Platform(Platform),
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The leading run of `s` whose bytes satisfy `pred` (Ruby's `\d+` or `\w+`
/// at that point), or `None` when it is empty.
fn leading(s: &str, pred: fn(u8) -> bool) -> Option<&str> {
    let n = s.bytes().take_while(|&b| pred(b)).count();
    (n > 0).then(|| &s[..n])
}

fn digits(s: &str) -> Option<&str> {
    leading(s, |b| b.is_ascii_digit())
}

/// `-?` before a capture that cannot start with `-`: the capture is read after
/// the dash if there is one, which is also what backtracking would find.
fn after_dash(s: &str) -> &str {
    s.strip_prefix('-').unwrap_or(s)
}

/// `\d+\.\d+` at the start of `s`.
fn dotted_pair(s: &str) -> Option<&str> {
    let major = digits(s)?;
    let minor = digits(s[major.len()..].strip_prefix('.')?)?;
    Some(&s[..major.len() + 1 + minor.len()])
}

/// `\d+(?:\.\d+)*` at the start of `s`.
fn dotted(s: &str) -> Option<&str> {
    let mut end = digits(s)?.len();
    while let Some(next) = s[end..].strip_prefix('.').and_then(digits) {
        end += 1 + next.len();
    }
    Some(&s[..end])
}

/// Where Ruby's `^` matches: the start, and after every newline.
fn line_starts(s: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(s.match_indices('\n').map(|(i, _)| i + 1))
}

/// Ruby's `$`: the end, or before a newline.
fn at_line_end(s: &str) -> bool {
    s.is_empty() || s.starts_with('\n')
}

/// What follows the first `lit` in `s` (`/lit.../` unanchored, when what comes
/// after `lit` cannot fail).
fn after_first<'a>(s: &'a str, lit: &str) -> Option<&'a str> {
    s.find(lit).map(|i| &s[i + lit.len()..])
}

/// What follows `lit` at the first line start that begins with it
/// (`/^lit.../`, when what comes after cannot fail).
fn after_line_start<'a>(s: &'a str, lit: &str) -> Option<&'a str> {
    line_starts(s).find_map(|p| s[p..].strip_prefix(lit))
}

/// `/lit(\d)/`: the digit after the first `lit` that has one.
fn digit_after<'a>(s: &'a str, lit: &str) -> Option<&'a str> {
    s.match_indices(lit)
        .find_map(|(i, _)| digits(&s[i + lit.len()..]).map(|d| &d[..1]))
}

/// `arch.sub(/-+$/, "")`: drop the first run of dashes that ends a line.
fn strip_trailing_dashes(arch: &str) -> String {
    let bytes = arch.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' {
            let end = i + arch[i..].bytes().take_while(|&b| b == b'-').count();
            if at_line_end(&arch[end..]) {
                return format!("{}{}", &arch[..i], &arch[end..]);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    arch.to_owned()
}

/// `cpu.match?(/i\d86/)`.
fn is_ix86(cpu: &str) -> bool {
    cpu.as_bytes()
        .windows(4)
        .any(|w| w[0] == b'i' && w[1].is_ascii_digit() && &w[2..] == b"86")
}

/// `/^dalvik-?(\d+)?$/`: `Some(version)` when it matches.
fn dalvik_dashed(os: &str) -> Option<Option<&str>> {
    line_starts(os).find_map(|p| {
        let rest = after_dash(os[p..].strip_prefix("dalvik")?);
        let version = digits(rest);
        at_line_end(&rest[version.map_or(0, str::len)..]).then_some(version)
    })
}

/// `/(mswin\d+)(?:[_-](\d+))?/`: the os and version.
fn mswin(os: &str) -> Option<(&str, Option<&str>)> {
    os.match_indices("mswin").find_map(|(i, _)| {
        let n = digits(&os[i + 5..])?.len();
        let name = &os[i..i + 5 + n];
        let rest = &os[i + 5 + n..];
        let version = rest.strip_prefix(['_', '-']).and_then(digits);
        Some((name, version))
    })
}

/// `/^(\w+_platform)-?(\d+)?/`, RubyGems' test platforms: `\w+` is greedy, so
/// the os ends at the last `_platform` in the leading word.
fn test_platform(os: &str) -> Option<(&str, Option<&str>)> {
    line_starts(os).find_map(|p| {
        let word = leading(&os[p..], is_word)?;
        let k = word.rfind("_platform").filter(|&k| k > 0)?;
        let name = &word[..k + "_platform".len()];
        Some((name, digits(after_dash(&os[p + name.len()..]))))
    })
}

/// The `case os` of `Gem::Platform#initialize`, in its order. `cpu_is_nil`
/// is what the `mswin` branch reads to decide that `mswin32` is x86.
fn os_and_version(os: Option<&str>, cpu_is_nil: bool) -> (String, Option<String>, bool) {
    let own = |s: &str| s.to_owned();
    let found = |name: &str, version: Option<&str>| Some((own(name), version.map(own)));
    let Some(os) = os else {
        return (own("unknown"), None, false);
    };
    let mut set_x86 = false;
    let result = if let Some(rest) = after_first(os, "aix") {
        found("aix", digits(after_dash(rest)))
    } else if os.contains("cygwin") {
        found("cygwin", None)
    } else if let Some(rest) = after_first(os, "darwin") {
        found("darwin", digits(after_dash(rest)))
    } else if os == "macruby" {
        found("macruby", None)
    } else if let Some(rest) = after_line_start(os, "macruby") {
        found("macruby", dotted(after_dash(rest)))
    } else if let Some(rest) = after_first(os, "freebsd") {
        found("freebsd", digits(after_dash(rest)))
    } else if os == "java" || os == "jruby" {
        found("java", None)
    } else if let Some(rest) = after_line_start(os, "java") {
        found("java", dotted(after_dash(rest)))
    } else if let Some(version) = dalvik_dashed(os) {
        found("dalvik", version)
    } else if line_starts(os).any(|p| os[p..].strip_prefix("dotnet").is_some_and(at_line_end)) {
        found("dotnet", None)
    } else if let Some(rest) = after_line_start(os, "dotnet") {
        found("dotnet", dotted(after_dash(rest)))
    } else if let Some(rest) = after_first(os, "linux") {
        found("linux", leading(after_dash(rest), is_word))
    } else if os.contains("mingw32") {
        found("mingw32", None)
    } else if let Some(rest) = after_first(os, "mingw") {
        found("mingw", leading(after_dash(rest), is_word))
    } else if let Some((name, version)) = mswin(os) {
        set_x86 = cpu_is_nil && name.ends_with("32");
        found(name, version)
    } else if os.contains("netbsdelf") {
        found("netbsdelf", None)
    } else if let Some(rest) = after_first(os, "openbsd") {
        found("openbsd", dotted_pair(after_dash(rest)))
    } else if let Some(rest) = after_first(os, "solaris") {
        found("solaris", dotted_pair(after_dash(rest)))
    } else if os.contains("wasi") {
        found("wasi", None)
    } else if let Some((name, version)) = test_platform(os) {
        found(name, version)
    } else {
        None
    };
    match result {
        Some((os, version)) => (os, version, set_x86),
        None => (own("unknown"), None, false),
    }
}

impl Platform {
    /// `Gem::Platform#initialize` with a String: `"x86_64-linux-gnu"`,
    /// `"arm64-darwin-24"`, `"java"`.
    pub fn parse(arch: &str) -> Platform {
        let arch = strip_trailing_dashes(arch);
        // `split("-", 2)`: nothing at all for an empty string.
        let mut fields = (!arch.is_empty())
            .then(|| arch.splitn(2, '-'))
            .into_iter()
            .flatten();
        let first = fields.next();
        let os = fields.next();
        let mut cpu = first.map(|c| if is_ix86(c) { "x86" } else { c }.to_owned());
        // A lone word is an os, not a cpu ("legacy jruby").
        let os = match os {
            Some(os) => Some(os),
            None => {
                cpu = None;
                first
            }
        };
        let (os, version, set_x86) = os_and_version(os, cpu.is_none());
        if set_x86 {
            cpu = Some("x86".to_owned());
        }
        Platform { cpu, os, version }
    }

    /// `Gem::Platform#initialize` with an Array: the parts as given, nothing
    /// normalized. RubyGems takes `nil` for the os too, but every match then
    /// raises, so the os is required here.
    pub fn from_parts(cpu: Option<&str>, os: &str, version: Option<&str>) -> Platform {
        Platform {
            cpu: cpu.map(str::to_owned),
            os: os.to_owned(),
            version: version.map(str::to_owned),
        }
    }

    /// `Gem::Platform.local` for a Ruby whose `RbConfig::CONFIG["arch"]` is
    /// `arch`: Windows' `mswin32`/`mswin64` gain the `_60` RubyGems assumes.
    /// `arch` is never `ruby`, `current` or empty, which `Gem::Platform.new`
    /// would turn into something other than a platform.
    pub fn local(arch: &str) -> Platform {
        let mswin = ["mswin32", "mswin64"].iter().any(|w| {
            arch.match_indices(w)
                .any(|(i, _)| at_line_end(&arch[i + w.len()..]))
        });
        if mswin {
            Platform::parse(&format!("{arch}_60"))
        } else {
            Platform::parse(arch)
        }
    }

    pub fn cpu(&self) -> Option<&str> {
        self.cpu.as_deref()
    }

    pub fn os(&self) -> &str {
        &self.os
    }

    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// `to_a`.
    pub fn to_a(&self) -> (Option<&str>, &str, Option<&str>) {
        (self.cpu(), self.os(), self.version())
    }

    /// `normalized_linux_version`: the libc with a leading `gnu` and a
    /// trailing `eabi`/`eabihf` taken off, `None` when nothing is left.
    pub fn normalized_linux_version(&self) -> Option<Cow<'_, str>> {
        let version = self.version.as_deref()?;
        let version = version.strip_prefix("gnu").unwrap_or(version);
        // `.sub(/eabi(hf)?\Z/, "")`: `\Z` is the end or before a final newline,
        // which a version read from a string never has but one given as parts may.
        let (body, newline) = match version.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (version, ""),
        };
        let stripped = body
            .strip_suffix("eabihf")
            .or_else(|| body.strip_suffix("eabi"));
        let version = match stripped {
            Some(body) if newline.is_empty() => Cow::Borrowed(body),
            Some(body) => Cow::Owned(format!("{body}{newline}")),
            None => Cow::Borrowed(version),
        };
        (!version.is_empty()).then_some(version)
    }

    /// `===`: may a gem built for `self` be installed on `runtime`? See the
    /// module notes for why the order matters.
    pub fn matches(&self, runtime: &Platform) -> bool {
        let wildcard = |cpu: Option<&str>| cpu.is_none_or(|c| c == "universal");
        if (self.cpu() == Some("universal") || runtime.cpu() == Some("universal"))
            && self.os.starts_with("mingw")
            && runtime.os.starts_with("mingw")
        {
            return true;
        }
        let cpu = wildcard(self.cpu())
            || wildcard(runtime.cpu())
            || self.cpu == runtime.cpu
            || (self.cpu() == Some("arm") && runtime.cpu().is_some_and(|c| c.starts_with("armv")));
        let unversioned =
            self.os != "linux" && (self.version.is_none() || runtime.version.is_none());
        // `["musl#{@version}", "musleabi#{@version}", "musleabihf#{@version}"]
        // .include?(other.version)`, where a missing version reads as "".
        let mine = self.version().unwrap_or("");
        let libc = self.os == "linux"
            && (self.normalized_linux_version() == runtime.normalized_linux_version()
                || runtime.version().is_some_and(|theirs| {
                    ["musl", "musleabi", "musleabihf"]
                        .iter()
                        .any(|musl| theirs.strip_prefix(musl) == Some(mine))
                }));
        let version = unversioned || libc || self.version == runtime.version;
        cpu && self.os == runtime.os && version
    }

    /// `=~` with a String: a few legacy spellings from 2007 are read by a
    /// table first, then the string goes through `Gem::Platform.new` (so
    /// `current` is `local`, and `ruby` matches nothing), then `===`.
    pub fn matches_str(&self, other: &str, local: &Platform) -> bool {
        let legacy = legacy_platform(other);
        let other = match legacy {
            Some(platform) => GemPlatform::Platform(platform),
            None => GemPlatform::new(Some(other), local),
        };
        match other {
            GemPlatform::Platform(other) => self.matches(&other),
            GemPlatform::Ruby => false,
        }
    }
}

/// The table at the top of `Gem::Platform#=~`, "from
/// http://gems.rubyforge.org/gems/yaml on 19 Aug 2007", in its order.
fn legacy_platform(other: &str) -> Option<Platform> {
    let parts = |cpu: Option<&str>, os: &str, version: Option<&str>| {
        Some(Platform::from_parts(cpu, os, version))
    };
    // `/^i686-darwin(\d)/`
    if let Some(d) = line_starts(other).find_map(|p| {
        let rest = other[p..].strip_prefix("i686-darwin")?;
        digits(rest).map(|d| &d[..1])
    }) {
        return parts(Some("x86"), "darwin", Some(d));
    }
    // `/^i\d86-linux/`
    if line_starts(other).any(|p| {
        let b = &other.as_bytes()[p..];
        b.len() >= 10 && b[0] == b'i' && b[1].is_ascii_digit() && &b[2..10] == b"86-linux"
    }) {
        return parts(Some("x86"), "linux", None);
    }
    if other == "java" || other == "jruby" {
        return parts(None, "java", None);
    }
    // `/^dalvik(\d+)?$/`
    if let Some(version) = line_starts(other).find_map(|p| {
        let rest = other[p..].strip_prefix("dalvik")?;
        let version = digits(rest);
        at_line_end(&rest[version.map_or(0, str::len)..]).then_some(version)
    }) {
        return parts(None, "dalvik", version);
    }
    // `/dotnet(\-(\d+\.\d+))?/`
    if let Some(rest) = after_first(other, "dotnet") {
        let version = rest.strip_prefix('-').and_then(dotted_pair);
        return parts(Some("universal"), "dotnet", version);
    }
    // `/mswin32(\_(\d+))?/`, `/mswin64(\_(\d+))?/`
    for (cpu, os) in [("x86", "mswin32"), ("x64", "mswin64")] {
        if let Some(rest) = after_first(other, os) {
            return parts(Some(cpu), os, rest.strip_prefix('_').and_then(digits));
        }
    }
    if other == "powerpc-darwin" {
        return parts(Some("powerpc"), "darwin", None);
    }
    // `/powerpc-darwin(\d)/`
    if let Some(d) = digit_after(other, "powerpc-darwin") {
        return parts(Some("powerpc"), "darwin", Some(d));
    }
    // `/sparc-solaris2.8/`, where `.` is any character but a newline.
    if other.match_indices("sparc-solaris2").any(|(i, _)| {
        let mut rest = other[i + "sparc-solaris2".len()..].chars();
        rest.next().is_some_and(|c| c != '\n') && rest.next() == Some('8')
    }) {
        return parts(Some("sparc"), "solaris", Some("2.8"));
    }
    // `/universal-darwin(\d)/`
    let d = digit_after(other, "universal-darwin")?;
    parts(Some("universal"), "darwin", Some(d))
}

/// `to_s`: the parts that are there, joined by `-`, or run together when there
/// is no cpu.
impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.cpu.is_none() { "" } else { "-" };
        let parts: Vec<&str> = [self.cpu(), Some(self.os()), self.version()]
            .into_iter()
            .flatten()
            .collect();
        f.write_str(&parts.join(sep))
    }
}

impl GemPlatform {
    /// `Gem::Platform.new`: `current` is the local platform; `ruby`, `nil` and
    /// `""` are `ruby`; anything else is parsed.
    pub fn new(arch: Option<&str>, local: &Platform) -> GemPlatform {
        match arch {
            Some("current") => GemPlatform::Platform(local.clone()),
            None | Some("ruby" | "") => GemPlatform::Ruby,
            Some(arch) => GemPlatform::Platform(Platform::parse(arch)),
        }
    }

    fn platform(&self) -> Option<&Platform> {
        match self {
            GemPlatform::Ruby => None,
            GemPlatform::Platform(p) => Some(p),
        }
    }
}

impl fmt::Display for GemPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GemPlatform::Ruby => f.write_str("ruby"),
            GemPlatform::Platform(p) => p.fmt(f),
        }
    }
}

/// `Gem::Platform.match_platforms?`: may a gem for `platform` be installed
/// where RubyGems accepts `platforms` (`Gem.platforms`, by default `ruby` and
/// the local platform)? `ruby` matches only `ruby`, by equality.
pub fn match_platforms(platform: &GemPlatform, platforms: &[GemPlatform]) -> bool {
    platforms.iter().any(|local| {
        local == platform
            || matches!((platform, local), (GemPlatform::Platform(p), GemPlatform::Platform(l)) if p.matches(l))
    })
}

/// `Gem::Platform.generic`: the first of `java`, `mswin32`, `mswin64` and
/// `universal-mingw` that `platform` matches, or `ruby`.
pub fn generic(platform: &GemPlatform) -> GemPlatform {
    let Some(platform) = platform.platform() else {
        return GemPlatform::Ruby;
    };
    [
        Platform::parse("java"),
        Platform::parse("mswin32"),
        Platform::parse("mswin64"),
        Platform::parse("universal-mingw"),
    ]
    .into_iter()
    .find(|generic| platform.matches(generic))
    .map_or(GemPlatform::Ruby, GemPlatform::Platform)
}

/// `Gem::Platform.platform_specificity_match`: lower is a better fit of a
/// gem for `spec` to a user on `user`. `-1` for the same platform,
/// 1,000,000 when either is `ruby`, otherwise os, cpu and version
/// differences weighted 1, 10 and 100.
pub fn platform_specificity_match(spec: &GemPlatform, user: &GemPlatform) -> i64 {
    if spec == user {
        return -1;
    }
    let (Some(spec), Some(user)) = (spec.platform(), user.platform()) else {
        return 1_000_000;
    };
    let os = i64::from(spec.os != user.os);
    let cpu = if spec.cpu == user.cpu
        || (spec.cpu() == Some("arm") && user.cpu().unwrap_or("").starts_with("arm"))
    {
        0
    } else if spec.cpu().is_none_or(|c| c == "universal") {
        1
    } else {
        2
    };
    let version = if spec.version == user.version {
        0
    } else if spec.version.is_none() {
        1
    } else {
        2
    };
    os + cpu * 10 + version * 100
}

/// `Gem::Platform.sort_best_platform_match`: a stable sort of candidates by
/// [`platform_specificity_match`] against `platform`.
pub fn sort_best_platform_match<T>(
    items: &mut [T],
    platform: &GemPlatform,
    platform_of: impl Fn(&T) -> &GemPlatform,
) {
    // `sort_by.with_index { [score, i] }`: Rust's sort is already stable.
    items.sort_by_key(|item| platform_specificity_match(platform_of(item), platform));
}

#[cfg(test)]
mod tests;

//! A stored gem's gemspec, as RubyGems reads it and writes it into a GEM_HOME
//! (#82, docs/package-manager.md, "Global store").
//!
//! A `.gem` carries its specification as YAML (`metadata.gz`, kept in the
//! store as `metadata`). `gem install` loads it with
//! `Gem::Specification.from_yaml` and writes `specifications/<full name>.gemspec`
//! with `Gem::Installer#write_spec`: `installed_by_version` set, then
//! `to_ruby_for_cache`. This is both halves ported from RubyGems 4.0.16, the
//! copy Spinel ships in `stdlib/rubygems/`, so the file is the one `gem
//! install` running on Spinel would write, byte for byte.
//!
//! It is Rust rather than RubyGems itself on Spinel's VM because the YAML
//! half needs Psych, a C extension Spinel does not have yet, and because a
//! warm install writes these for every gem and should not boot a VM to do it.
//!
//! The load follows Psych's `ToRuby` for the objects a gemspec holds:
//! `!ruby/object:Gem::Specification`, `Gem::Version`, `Gem::Requirement`
//! (and its old name `Gem::Version::Requirement`), `Gem::Dependency` and
//! `Gem::Platform`, the classes `Gem::SafeYAML` permits; plain scalars are
//! typed by Psych's `ScalarScanner#tokenize`, and anchors and aliases are
//! followed. Then `from_yaml`'s own steps: `specification_version ||= -1`,
//! `reset_nil_attributes_to_default`, `flatten_require_paths`. The writer
//! follows `Gem::Specification#to_ruby` attribute by attribute, through the
//! same getters (`files` is rebuilt from the executables, extensions and rdoc
//! files once `for_cache` has emptied it) and `ruby_code`, `String#dump`
//! included.

use std::fmt::Write as _;

use yaml_rust2::parser::{Event, EventReceiver, Parser, Tag};
use yaml_rust2::scanner::TScalarStyle;

use crate::gem_platform::Platform;
use crate::gem_version;

/// `Gem::VERSION` of the RubyGems Spinel ships: what an installed gemspec
/// records as `installed_by_version`, and a missing `rubygems_version`
/// defaults to. A test checks it against `stdlib/rubygems.rb`.
pub const RUBYGEMS_VERSION: &str = "4.0.16";

/// A Ruby object Psych made from the YAML.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Nil,
    Bool(bool),
    Int(i128),
    Float(f64),
    /// A String, UTF-8 as Psych reads one.
    Str(String),
    /// A binary (ASCII-8BIT) String, from `!binary`.
    Bytes(Vec<u8>),
    Sym(String),
    /// A Time or a Date: only the day matters to a gemspec, which keeps the
    /// date at one-day resolution. `(year, month, day)`.
    Day(i64, u32, u32),
    Seq(Vec<Value>),
    /// A Hash (`class` is `None`), or an object of a `!ruby/object:` class
    /// with its instance variables.
    Map {
        class: Option<String>,
        entries: Vec<(Value, Value)>,
    },
}

impl Value {
    fn object(&self, name: &str) -> bool {
        matches!(self, Value::Map { class: Some(c), .. } if c == name)
    }

    fn requirement(&self) -> bool {
        self.object("Gem::Requirement") || self.object("Gem::Version::Requirement")
    }

    /// An instance variable of an object, or a Hash's value for a String
    /// key.
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map { entries, .. } => entries
                .iter()
                .rev()
                .find(|(k, _)| matches!(k, Value::Str(s) if s == key))
                .map(|(_, v)| v),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }
}

/// A gemspec loaded from its YAML, as `Gem::Specification.from_yaml` leaves
/// it.
#[derive(Debug)]
pub struct Spec {
    /// The instance variables, by name without `@`.
    ivars: Vec<(String, Value)>,
    /// `@original_platform`, as `platform=` was given it.
    original_platform: Option<Value>,
    /// `@new_platform`: `None` for `Gem::Platform::RUBY`.
    platform: Option<PlatformValue>,
    /// `@date`, as `date=` coerced it; `None` until it is set.
    date: Option<(i64, u32, u32)>,
}

#[derive(Debug, Clone)]
enum PlatformValue {
    Parsed(Platform),
    /// A `!ruby/object:Gem::Platform`: its cpu, os and version.
    Object(Value),
}

impl PlatformValue {
    fn to_s(&self) -> Result<String, String> {
        match self {
            PlatformValue::Parsed(p) => Ok(p.to_string()),
            PlatformValue::Object(v) => {
                let parts = platform_parts(v)?;
                let sep = if parts[0].is_none() { "" } else { "-" };
                Ok(parts.into_iter().flatten().collect::<Vec<_>>().join(sep))
            }
        }
    }
}

fn platform_parts(v: &Value) -> Result<[Option<String>; 3], String> {
    let mut parts: [Option<String>; 3] = [None, None, None];
    for (i, key) in ["cpu", "os", "version"].iter().enumerate() {
        parts[i] = match v.get(key) {
            None | Some(Value::Nil) => None,
            Some(other) => Some(to_s(other)?),
        };
    }
    Ok(parts)
}

/// `Gem::Specification.from_yaml`, or why RubyGems would refuse the YAML.
pub fn from_yaml(yaml: &[u8]) -> Result<Spec, String> {
    let text = std::str::from_utf8(yaml).map_err(|_| "the gemspec is not UTF-8".to_owned())?;
    let root = load(&normalize_yaml_input(text))?;
    let Value::Map {
        class: Some(class),
        entries,
    } = root
    else {
        return Err("YAML data doesn't evaluate to gem specification".to_owned());
    };
    if class != "Gem::Specification" {
        return Err("YAML data doesn't evaluate to gem specification".to_owned());
    }
    let mut spec = Spec {
        ivars: Vec::new(),
        original_platform: None,
        platform: None,
        date: None,
    };
    // `yaml_initialize`: every key an instance variable, except `date` and
    // `platform`, which go through their setters.
    for (key, value) in entries {
        let key = to_s(&key)?;
        match key.as_str() {
            "date" => spec.date = Some(coerce_date(&value)?),
            "platform" => spec.set_platform(value)?,
            _ => spec.set(&key, value),
        }
    }
    if !spec
        .ivar("specification_version")
        .is_some_and(Value::truthy)
    {
        spec.set("specification_version", Value::Int(-1));
    }
    // `reset_nil_attributes_to_default`, for the attributes whose default is
    // not nil.
    let defaults = [
        ("authors", Value::Seq(Vec::new())),
        ("bindir", Value::Str("bin".to_owned())),
        ("cert_chain", Value::Seq(Vec::new())),
        ("dependencies", Value::Seq(Vec::new())),
        ("executables", Value::Seq(Vec::new())),
        ("extensions", Value::Seq(Vec::new())),
        ("extra_rdoc_files", Value::Seq(Vec::new())),
        ("files", Value::Seq(Vec::new())),
        ("licenses", Value::Seq(Vec::new())),
        (
            "metadata",
            Value::Map {
                class: None,
                entries: Vec::new(),
            },
        ),
        ("platform", Value::Str("ruby".to_owned())),
        ("rdoc_options", Value::Seq(Vec::new())),
        (
            "require_paths",
            Value::Seq(vec![Value::Str("lib".to_owned())]),
        ),
        ("required_ruby_version", default_requirement()),
        ("required_rubygems_version", default_requirement()),
        ("requirements", Value::Seq(Vec::new())),
        ("rubygems_version", Value::Str(RUBYGEMS_VERSION.to_owned())),
        ("specification_version", Value::Int(4)),
        ("test_files", Value::Seq(Vec::new())),
    ];
    for (name, default) in defaults {
        if matches!(spec.ivar(name), None | Some(Value::Nil)) {
            spec.set(name, default);
        }
    }
    // `flatten_require_paths`.
    if let Some(Value::Seq(paths)) = spec.ivar("require_paths")
        && matches!(paths.first(), Some(Value::Seq(_)))
    {
        let mut flat = Vec::new();
        flatten(paths, &mut flat);
        spec.set("require_paths", Value::Seq(flat));
    }
    Ok(spec)
}

/// `Gem::Specification.normalize_yaml_input`.
fn normalize_yaml_input(input: &str) -> String {
    let mut result = if input.starts_with("--- ") {
        input.to_owned()
    } else {
        format!("--- {input}")
    };
    result = result.replace(" !!null \n", " \n");
    // `date: 2011-04-26 00:00:00.000000000Z` gains the space Psych wants
    // before the `Z`.
    let mut fixed = String::with_capacity(result.len());
    for line in result.split_inclusive('\n') {
        match date_without_space(line) {
            Some(at) => {
                fixed.push_str(&line[..at]);
                fixed.push(' ');
                fixed.push_str(&line[at..]);
            }
            None => fixed.push_str(line),
        }
    }
    fixed
}

/// Where `/^(date: \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d+?)Z/` puts its
/// space: before the `Z`, when the line is shaped so.
fn date_without_space(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let shape = b"date: dddd-dd-dd dd:dd:dd.";
    if b.len() < shape.len() + 2 {
        return None;
    }
    for (i, &s) in shape.iter().enumerate() {
        let ok = if s == b'd' {
            b[i].is_ascii_digit()
        } else {
            b[i] == s
        };
        if !ok {
            return None;
        }
    }
    let mut at = shape.len();
    let digits = at;
    while at < b.len() && b[at].is_ascii_digit() {
        at += 1;
    }
    (at > digits && b.get(at) == Some(&b'Z')).then_some(at)
}

fn default_requirement() -> Value {
    requirement_of(">=", "0")
}

fn requirement_of(op: &str, version: &str) -> Value {
    Value::Map {
        class: Some("Gem::Requirement".to_owned()),
        entries: vec![(
            Value::Str("requirements".to_owned()),
            Value::Seq(vec![Value::Seq(vec![
                Value::Str(op.to_owned()),
                Value::Map {
                    class: Some("Gem::Version".to_owned()),
                    entries: vec![(
                        Value::Str("version".to_owned()),
                        Value::Str(version.to_owned()),
                    )],
                },
            ])]),
        )],
    }
}

impl Spec {
    fn ivar(&self, name: &str) -> Option<&Value> {
        self.ivars.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    fn set(&mut self, name: &str, value: Value) {
        match self.ivars.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = value,
            None => self.ivars.push((name.to_owned(), value)),
        }
    }

    /// An attribute read through an `@x ||= []` getter.
    fn list(&self, name: &str) -> Value {
        match self.ivar(name) {
            Some(v) if v.truthy() => v.clone(),
            _ => Value::Seq(Vec::new()),
        }
    }

    fn plain(&self, name: &str) -> Value {
        self.ivar(name).cloned().unwrap_or(Value::Nil)
    }

    /// `platform=`.
    fn set_platform(&mut self, value: Value) -> Result<(), String> {
        self.original_platform = Some(value.clone());
        self.platform = match &value {
            Value::Nil => None,
            Value::Str(s) if s == "ruby" => None,
            Value::Str(s) if s == "current" => {
                return Err("a gemspec for the platform \"current\"".to_owned());
            }
            Value::Str(s) if s == "mswin32" => {
                Some(PlatformValue::Parsed(Platform::parse("x86-mswin32")))
            }
            Value::Str(s) if s == "i586-linux" => {
                Some(PlatformValue::Parsed(Platform::parse("x86-linux")))
            }
            Value::Str(s) if s == "powerpc-darwin" => {
                Some(PlatformValue::Parsed(Platform::parse("ppc-darwin")))
            }
            Value::Str(s) => Some(PlatformValue::Parsed(Platform::parse(s))),
            v if v.object("Gem::Platform") => Some(PlatformValue::Object(v.clone())),
            other => return Err(format!("a platform of {other:?}")),
        };
        let platform = self.platform_s()?;
        self.set("platform", Value::Str(platform));
        Ok(())
    }

    /// `platform.to_s`.
    fn platform_s(&self) -> Result<String, String> {
        match &self.platform {
            None => Ok("ruby".to_owned()),
            Some(p) => p.to_s(),
        }
    }

    pub fn name(&self) -> Result<String, String> {
        to_s(&self.plain("name"))
    }

    pub fn version(&self) -> Result<String, String> {
        to_s(&self.plain("version"))
    }

    /// `full_name`: `name-version`, and `-platform` unless it is `ruby`.
    pub fn full_name(&self) -> Result<String, String> {
        let (name, version) = (self.name()?, self.version()?);
        Ok(match &self.platform {
            None => format!("{name}-{version}"),
            Some(p) => format!("{name}-{version}-{}", p.to_s()?),
        })
    }

    /// `bindir`, or `None` when it is nil.
    pub fn bindir(&self) -> Result<Option<String>, String> {
        match self.plain("bindir") {
            Value::Nil | Value::Bool(false) => Ok(None),
            v => to_s(&v).map(Some),
        }
    }

    /// `executables`.
    pub fn executables(&self) -> Result<Vec<String>, String> {
        strings(&self.list("executables"))
    }

    /// `extensions`.
    pub fn extensions(&self) -> Result<Vec<String>, String> {
        strings(&self.list("extensions"))
    }

    /// `Gem::Installer#verify_spec`, which refuses what would put code in
    /// the installed gemspec's text or a file outside the GEM_HOME: a name
    /// RubyGems does not allow, a line break in a require path, extension or
    /// the platform (they are written into a comment), a
    /// `specification_version` that is not digits, a dependency whose type is
    /// neither runtime nor development or whose name has a line break, `<` or
    /// `>` (it is written into `%q<>`), an executable that is not a plain file
    /// name, and a `bindir` outside the gem.
    pub fn verify(&self) -> Result<(), String> {
        let name = self.name()?;
        let ok_name = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
        if !matches!(self.plain("name"), Value::Str(_)) || !ok_name {
            return Err("an invalid name".to_owned());
        }
        if strings(&self.plain("require_paths"))?
            .iter()
            .any(|p| line_break(p))
        {
            return Err("an invalid require_paths".to_owned());
        }
        if self.extensions()?.iter().any(|e| line_break(e)) {
            return Err("an invalid extensions".to_owned());
        }
        if line_break(&self.platform_s()?) {
            return Err("an invalid platform".to_owned());
        }
        let version = to_s(&self.plain("specification_version"))?;
        if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
            return Err("an invalid specification_version".to_owned());
        }
        let Value::Seq(dependencies) = self.list("dependencies") else {
            return Err("an invalid dependencies".to_owned());
        };
        for dep in &dependencies {
            let kind = match dep.get("type") {
                None | Some(Value::Nil) => "runtime".to_owned(),
                Some(Value::Sym(t)) => t.clone(),
                Some(_) => String::new(),
            };
            let name = to_s(dep.get("name").unwrap_or(&Value::Nil))?;
            if !matches!(kind.as_str(), "runtime" | "development")
                || line_break(&name)
                || name.contains(['<', '>'])
            {
                return Err("an invalid dependencies".to_owned());
            }
        }
        let Value::Seq(executables) = self.list("executables") else {
            return Err("an invalid executable".to_owned());
        };
        for exe in &executables {
            let Value::Str(exe) = exe else {
                return Err("an invalid executable".to_owned());
            };
            if exe.is_empty() || exe.contains('/') || exe == "." || exe == ".." || line_break(exe) {
                return Err("an invalid executable".to_owned());
            }
        }
        let Value::Str(bindir) = self.plain("bindir") else {
            return Err("an invalid bindir".to_owned());
        };
        // `File.expand_path(File.join(gem_dir, bindir))` must stay in the gem.
        let mut depth: usize = 0;
        for part in bindir.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .ok_or_else(|| "an invalid bindir".to_owned())?;
                }
                _ => depth += 1,
            }
        }
        Ok(())
    }

    /// The installed gemspec: `installed_by_version = Gem.rubygems_version`
    /// and `to_ruby_for_cache`, as `Gem::Installer#write_spec` writes it.
    pub fn to_ruby_for_cache(&self) -> Result<String, String> {
        // `for_cache`: `files = nil` and `test_files = nil`, which their
        // setters make empty Arrays.
        let mut lines: Vec<String> = Vec::new();
        let name = self.plain("name");
        let version = self.plain("version");
        let require_paths = self.plain("require_paths");
        let extensions = self.list("extensions");
        lines.push("# -*- encoding: utf-8 -*-".to_owned());
        lines.push(format!(
            "# stub: {} {} {} {}",
            to_s(&name)?,
            to_s(&version)?,
            self.platform_s()?,
            join(&require_paths, "\0")?
        ));
        if !matches!(&extensions, Value::Seq(e) if e.is_empty()) {
            lines.push(format!("# stub: {}", join(&extensions, "\0")?));
        }
        lines.push(String::new());
        lines.push("Gem::Specification.new do |s|".to_owned());
        lines.push(format!("  s.name = {}", ruby_code(&name)?));
        lines.push(format!("  s.version = {}", ruby_code(&version)?));
        if self.platform.is_some() {
            let original = match &self.original_platform {
                Some(v) if v.truthy() => v.clone(),
                _ => Value::Str(self.platform_s()?),
            };
            let code = match &original {
                v if v.object("Gem::Platform") => {
                    let parts = platform_parts(v)?
                        .into_iter()
                        .map(|p| p.map_or(Value::Nil, Value::Str))
                        .collect();
                    format!("Gem::Platform.new({})", ruby_code(&Value::Seq(parts))?)
                }
                v => ruby_code(v)?,
            };
            lines.push(format!("  s.platform = {code}"));
        }
        lines.push(String::new());
        lines.push(format!(
            "  s.required_rubygems_version = {} if s.respond_to? :required_rubygems_version=",
            ruby_code(&self.plain("required_rubygems_version"))?
        ));
        let metadata = self.plain("metadata");
        let empty = match &metadata {
            Value::Map { entries, .. } => entries.is_empty(),
            Value::Seq(items) => items.is_empty(),
            Value::Str(s) => s.is_empty(),
            _ => false,
        };
        if metadata.truthy() && !empty {
            lines.push(format!(
                "  s.metadata = {} if s.respond_to? :metadata=",
                ruby_code(&metadata)?
            ));
        }
        lines.push(format!(
            "  s.require_paths = {}",
            ruby_code(&require_paths)?
        ));

        let seq = || Value::Seq(Vec::new());
        let nil = || Value::Nil;
        // `@@attributes` in order, less the ones `to_ruby` writes itself,
        // each with its default and whether it is required.
        #[expect(clippy::type_complexity, reason = "a table, read once")]
        let attributes: [(&str, fn(&Spec) -> Result<Value, String>, Value, bool); 20] = [
            ("authors", |s| Ok(s.list("authors")), seq(), false),
            ("autorequire", |s| Ok(s.plain("autorequire")), nil(), false),
            (
                "bindir",
                |s| Ok(s.plain("bindir")),
                Value::Str("bin".to_owned()),
                false,
            ),
            ("cert_chain", |s| Ok(s.plain("cert_chain")), seq(), false),
            ("date", |s| Ok(s.date()), nil(), true),
            ("description", |s| Ok(s.plain("description")), nil(), false),
            ("email", |s| Ok(s.plain("email")), nil(), false),
            ("executables", |s| Ok(s.list("executables")), seq(), false),
            ("extensions", |s| Ok(s.list("extensions")), seq(), false),
            (
                "extra_rdoc_files",
                |s| Ok(s.list("extra_rdoc_files")),
                seq(),
                false,
            ),
            ("files", Spec::files_for_cache, seq(), false),
            ("homepage", |s| Ok(s.plain("homepage")), nil(), false),
            ("licenses", |s| Ok(s.list("licenses")), seq(), false),
            (
                "post_install_message",
                |s| Ok(s.plain("post_install_message")),
                nil(),
                false,
            ),
            ("rdoc_options", |s| Ok(s.list("rdoc_options")), seq(), false),
            (
                "required_ruby_version",
                |s| Ok(s.plain("required_ruby_version")),
                default_requirement(),
                false,
            ),
            ("requirements", |s| Ok(s.list("requirements")), seq(), false),
            (
                "rubygems_version",
                |s| Ok(s.plain("rubygems_version")),
                Value::Str(RUBYGEMS_VERSION.to_owned()),
                true,
            ),
            ("summary", |s| Ok(s.plain("summary")), nil(), true),
            ("test_files", Spec::test_files_for_cache, seq(), false),
        ];
        for (attribute, get, default, required) in attributes {
            let current = get(self)?;
            if required || !ruby_eq(&current, &default) {
                lines.push(format!("  s.{attribute} = {}", ruby_code(&current)?));
            }
        }
        if let Some(Value::Str(key)) = self.ivar("signing_key") {
            lines.push(format!(
                "  s.signing_key = {}",
                ruby_code(&Value::Str(key.clone()))?
            ));
        }
        // `installed_by_version` is set, so its line is always written.
        lines.push(String::new());
        lines.push(format!(
            "  s.installed_by_version = {}",
            ruby_code(&Value::Str(RUBYGEMS_VERSION.to_owned()))?
        ));
        let dependencies = match self.list("dependencies") {
            Value::Seq(d) => d,
            other => return Err(format!("dependencies of {other:?}")),
        };
        if !dependencies.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "  s.specification_version = {}",
                to_s(&self.plain("specification_version"))?
            ));
            lines.push(String::new());
            for dep in &dependencies {
                lines.push(dependency_line(dep)?);
            }
        }
        lines.push("end".to_owned());
        lines.push(String::new());
        Ok(lines.join("\n"))
    }

    /// `date`: the coerced `@date`, else `Gem.source_date_epoch`'s day.
    fn date(&self) -> Value {
        let (y, m, d) = self.date.unwrap_or_else(source_date_epoch_day);
        Value::Day(y, m, d)
    }

    /// `files` once `for_cache` has set `@files` and `@test_files` to `[]`:
    /// `[add_bindir(@executables), @extra_rdoc_files,
    /// @extensions].flatten.compact.uniq.sort`.
    fn files_for_cache(&self) -> Result<Value, String> {
        let mut parts = Vec::new();
        if let Some(executables) = self.ivar("executables").filter(|v| v.truthy()) {
            match self.ivar("bindir").filter(|v| v.truthy()) {
                Some(bindir) => {
                    let items = match executables {
                        Value::Seq(items) => items.clone(),
                        other => vec![other.clone()],
                    };
                    // `File.join` raising (a non-String) is rescued to nil.
                    let mut joined = Vec::new();
                    let mut ok = true;
                    for item in &items {
                        match (bindir, item) {
                            (Value::Str(b), Value::Str(e)) => {
                                joined.push(Value::Str(file_join(b, e)));
                            }
                            _ => ok = false,
                        }
                    }
                    if ok {
                        parts.push(Value::Seq(joined));
                    }
                }
                None => parts.push(executables.clone()),
            }
        }
        parts.push(self.plain("extra_rdoc_files"));
        parts.push(self.plain("extensions"));
        let mut flat = Vec::new();
        flatten(&parts, &mut flat);
        flat.retain(|v| *v != Value::Nil);
        let mut unique: Vec<Value> = Vec::new();
        for v in flat {
            if !unique.iter().any(|u| ruby_eql(u, &v)) {
                unique.push(v);
            }
        }
        let mut keyed = Vec::new();
        for v in unique {
            match &v {
                Value::Str(s) => keyed.push((s.as_bytes().to_vec(), v)),
                Value::Bytes(b) => keyed.push((b.clone(), v)),
                other => return Err(format!("files that do not sort: {other:?}")),
            }
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Value::Seq(keyed.into_iter().map(|(_, v)| v).collect()))
    }

    /// `test_files` once `for_cache` has set `@test_files` to `[]`: an old
    /// gem's `@test_suite_file` still takes its place.
    fn test_files_for_cache(&self) -> Result<Value, String> {
        Ok(match self.ivar("test_suite_file") {
            Some(file) => {
                let mut flat = Vec::new();
                flatten(std::slice::from_ref(file), &mut flat);
                Value::Seq(flat)
            }
            None => Value::Seq(Vec::new()),
        })
    }
}

/// `"  s.add_#{dep.type}_dependency(%q<#{dep.name}>.freeze, #{ruby_code dep.requirements_list})"`.
fn dependency_line(dep: &Value) -> Result<String, String> {
    if !dep.object("Gem::Dependency") {
        return Err(format!("a dependency of {dep:?}"));
    }
    let name = to_s(dep.get("name").unwrap_or(&Value::Nil))?;
    let kind = match dep.get("type") {
        None | Some(Value::Nil) => "runtime".to_owned(),
        Some(t) => to_s(t)?,
    };
    // `requirement`: `@requirement`, else the older `@version_requirements`.
    let requirement = match (dep.get("requirement"), dep.get("version_requirements")) {
        (Some(r), _) if r.truthy() => r,
        (_, Some(r)) => r,
        _ => return Err(format!("dependency {name} has no requirement")),
    };
    let list = as_list(requirement)?;
    Ok(format!(
        "  s.add_{kind}_dependency(%q<{name}>.freeze, {})",
        ruby_code(&Value::Seq(list.into_iter().map(Value::Str).collect()))?
    ))
}

/// Whether `/\R/` matches: any line break Ruby knows.
fn line_break(s: &str) -> bool {
    s.contains([
        '\n', '\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}',
    ])
}

/// `File.join` of two Strings.
fn file_join(a: &str, b: &str) -> String {
    match (a.ends_with('/'), b.starts_with('/')) {
        (true, true) => format!("{a}{}", &b[1..]),
        (false, false) if !a.is_empty() => format!("{a}/{b}"),
        _ => format!("{a}{b}"),
    }
}

fn flatten(items: &[Value], out: &mut Vec<Value>) {
    for item in items {
        match item {
            Value::Seq(inner) => flatten(inner, out),
            other => out.push(other.clone()),
        }
    }
}

fn strings(v: &Value) -> Result<Vec<String>, String> {
    match v {
        Value::Seq(items) => items.iter().map(to_s).collect(),
        other => Ok(vec![to_s(other)?]),
    }
}

fn join(v: &Value, sep: &str) -> Result<String, String> {
    match v {
        Value::Seq(items) => Ok(items
            .iter()
            .map(to_s)
            .collect::<Result<Vec<_>, _>>()?
            .join(sep)),
        other => Err(format!("cannot join {other:?}")),
    }
}

/// `Gem::Requirement#as_list`: `"#{op} #{version}"` for each.
fn as_list(req: &Value) -> Result<Vec<String>, String> {
    if !req.requirement() {
        return Err(format!("a requirement of {req:?}"));
    }
    let Some(Value::Seq(list)) = req.get("requirements") else {
        return Err("a requirement with no requirements".to_owned());
    };
    list.iter()
        .map(|pair| match pair {
            Value::Seq(pair) if pair.len() == 2 => {
                Ok(format!("{} {}", to_s(&pair[0])?, to_s(&pair[1])?))
            }
            other => Err(format!("a requirement of {other:?}")),
        })
        .collect()
}

/// `!=`'s opposite for the values a gemspec attribute is compared with its
/// default by.
fn ruby_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Str(x), Value::Bytes(y)) | (Value::Bytes(y), Value::Str(x)) => {
            x.is_ascii() && x.as_bytes() == y.as_slice()
        }
        (Value::Int(x), Value::Float(y)) | (Value::Float(y), Value::Int(x)) => *x as f64 == *y,
        (Value::Seq(x), Value::Seq(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| ruby_eq(a, b))
        }
        (x, y) if x.requirement() && y.requirement() => requirement_eq(x, y),
        (x, y) if x.object("Gem::Version") && y.object("Gem::Version") => version_eq(x, y),
        (Value::Map { class: None, .. }, Value::Map { class: None, .. }) => a == b,
        (Value::Map { .. }, _) | (_, Value::Map { .. }) => false,
        _ => a == b,
    }
}

/// `eql?`, which `uniq` compares by: no conversions.
fn ruby_eql(a: &Value, b: &Value) -> bool {
    a == b
}

fn version_eq(a: &Value, b: &Value) -> bool {
    let text = |v: &Value| v.get("version").and_then(|v| to_s(v).ok());
    match (text(a), text(b)) {
        (Some(x), Some(y)) => {
            x == y
                || match (gem_version::Version::new(&x), gem_version::Version::new(&y)) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                }
        }
        _ => false,
    }
}

/// `Gem::Requirement#==` for the default it is compared with: the same
/// requirements, sorted, with each version equal by `<=>`; a `~>` one must
/// also be spelled alike.
fn requirement_eq(a: &Value, b: &Value) -> bool {
    let pairs = |v: &Value| -> Option<Vec<(Value, Value)>> {
        let Some(Value::Seq(list)) = v.get("requirements") else {
            return None;
        };
        let mut pairs: Vec<(String, (Value, Value))> = Vec::new();
        for pair in list {
            let Value::Seq(pair) = pair else { return None };
            if pair.len() != 2 {
                return None;
            }
            let key = format!("{:?}", (to_s(&pair[0]).ok(), to_s(&pair[1]).ok()));
            pairs.push((key, (pair[0].clone(), pair[1].clone())));
        }
        pairs.sort_by(|x, y| x.0.cmp(&y.0));
        Some(pairs.into_iter().map(|(_, p)| p).collect())
    };
    let (Some(x), Some(y)) = (pairs(a), pairs(b)) else {
        return false;
    };
    x.len() == y.len()
        && x.iter().zip(&y).all(|((xo, xv), (yo, yv))| {
            ruby_eq(xo, yo)
                && version_eq(xv, yv)
                && (to_s(xo).ok().as_deref() != Some("~>")
                    || xv.get("version") == yv.get("version"))
        })
}

/// `to_s` of the values a gemspec writes into its text.
fn to_s(v: &Value) -> Result<String, String> {
    Ok(match v {
        Value::Nil => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => float_to_s(*f),
        Value::Str(s) => s.clone(),
        Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        Value::Sym(s) => s.clone(),
        v if v.object("Gem::Version") => to_s(v.get("version").unwrap_or(&Value::Nil))?,
        v if v.requirement() => as_list(v)?.join(", "),
        other => return Err(format!("no to_s for {other:?}")),
    })
}

/// `Float#to_s` (and `inspect`): the shortest digits that read back, with
/// Ruby's `.0`, and exponents from 1e16 and below 1e-4.
fn float_to_s(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        let e = format!("{f:e}");
        let (mantissa, exponent) = e.split_once('e').expect("{:e} has an e");
        let mantissa = if mantissa.contains('.') {
            mantissa.to_owned()
        } else {
            format!("{mantissa}.0")
        };
        let exponent: i32 = exponent.parse().expect("an exponent");
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exponent.abs());
    }
    let s = format!("{f}");
    if s.contains('.') { s } else { format!("{s}.0") }
}

/// `Gem::Specification#ruby_code`.
fn ruby_code(v: &Value) -> Result<String, String> {
    Ok(match v {
        Value::Str(s) => dump(s.as_bytes(), false) + ".freeze",
        Value::Bytes(b) => dump(b, true) + ".freeze",
        Value::Seq(items) => format!(
            "[{}]",
            items
                .iter()
                .map(ruby_code)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        ),
        Value::Map {
            class: None,
            entries,
        } => {
            let mut keyed = Vec::new();
            for (k, val) in entries {
                if !keyed.iter().any(|(key, _): &(Value, Value)| key == k) {
                    let last = entries
                        .iter()
                        .rev()
                        .find(|(key, _)| key == k)
                        .map(|(_, v)| v);
                    keyed.push((k.clone(), last.unwrap_or(val).clone()));
                }
            }
            let mut sortable = Vec::new();
            for (k, val) in keyed {
                let key = match &k {
                    Value::Str(s) => s.as_bytes().to_vec(),
                    Value::Bytes(b) => b.clone(),
                    Value::Sym(s) => s.as_bytes().to_vec(),
                    other => return Err(format!("a metadata key of {other:?}")),
                };
                sortable.push((key, k, val));
            }
            sortable.sort_by(|a, b| a.0.cmp(&b.0));
            let mut seg = Vec::new();
            for (_, k, val) in sortable {
                seg.push(format!(
                    "{} => {}",
                    dump(to_s(&k)?.as_bytes(), false),
                    dump(to_s(&val)?.as_bytes(), false)
                ));
            }
            format!("{{ {} }}", seg.join(", "))
        }
        v if v.object("Gem::Version") => ruby_code(&Value::Str(to_s(v)?))?,
        Value::Day(y, m, d) => dump(format!("{y:04}-{m:02}-{d:02}").as_bytes(), false),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => float_to_s(*f),
        Value::Bool(b) => b.to_string(),
        Value::Nil => "nil".to_owned(),
        v if v.object("Gem::Platform") => {
            let parts = platform_parts(v)?
                .into_iter()
                .map(|p| p.map_or(Value::Nil, Value::Str))
                .collect();
            format!("Gem::Platform.new({})", ruby_code(&Value::Seq(parts))?)
        }
        v if v.requirement() => {
            let list = as_list(v)?;
            let arg = if list.len() == 1 {
                Value::Str(list.join(", "))
            } else {
                Value::Seq(list.into_iter().map(Value::Str).collect())
            };
            format!("Gem::Requirement.new({})", ruby_code(&arg)?)
        }
        other => return Err(format!("ruby_code case not handled: {other:?}")),
    })
}

/// `String#dump`: a double-quoted literal that reads back to the same bytes.
/// A UTF-8 String writes its other characters as `\u`; a binary one, and
/// any byte that is not UTF-8, as `\x`.
pub fn dump(bytes: &[u8], binary: bool) -> String {
    let mut out = String::from("\"");
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'#' if matches!(bytes.get(i + 1), Some(b'{' | b'$' | b'@')) => out.push_str("\\#"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x0c => out.push_str("\\f"),
            0x0b => out.push_str("\\v"),
            0x08 => out.push_str("\\b"),
            0x07 => out.push_str("\\a"),
            0x1b => out.push_str("\\e"),
            0x20..=0x7e => out.push(b as char),
            0x00..=0x7f => {
                let _ = write!(out, "\\x{b:02X}");
            }
            _ if !binary => {
                let len = match b {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 0,
                };
                match bytes
                    .get(i..i + len)
                    .and_then(|c| std::str::from_utf8(c).ok())
                    .and_then(|c| c.chars().next())
                {
                    Some(c) if len > 0 => {
                        let cp = c as u32;
                        if cp > 0xffff {
                            let _ = write!(out, "\\u{{{cp:X}}}");
                        } else {
                            let _ = write!(out, "\\u{cp:04X}");
                        }
                        i += len;
                        continue;
                    }
                    _ => {
                        let _ = write!(out, "\\x{b:02X}");
                    }
                }
            }
            _ => {
                let _ = write!(out, "\\x{b:02X}");
            }
        }
        i += 1;
    }
    out.push('"');
    out
}

/// `date=`: a Time or Date keeps its day; a String must look like a date;
/// anything else is `TODAY`.
fn coerce_date(v: &Value) -> Result<(i64, u32, u32), String> {
    match v {
        Value::Day(y, m, d) => Ok((*y, *m, *d)),
        Value::Str(s) => date_time_format(s)
            .ok_or_else(|| format!("invalid date format in specification: {s:?}")),
        _ => Ok(today()),
    }
}

/// `DateTimeFormat`: `YYYY-MM-DD`, optionally followed by a time with
/// fractional seconds and a zone.
fn date_time_format(s: &str) -> Option<(i64, u32, u32)> {
    // `\Z` allows one trailing newline.
    let s = s.strip_suffix('\n').unwrap_or(s);
    let b = s.as_bytes();
    if b.len() < 10
        || !b[..4].iter().all(u8::is_ascii_digit)
        || b[4] != b'-'
        || !b[5..7].iter().all(u8::is_ascii_digit)
        || b[7] != b'-'
        || !b[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let rest = &s[10..];
    if !rest.is_empty() && !time_and_zone(rest) {
        return None;
    }
    Some((
        s[..4].parse().ok()?,
        s[5..7].parse().ok()?,
        s[8..10].parse().ok()?,
    ))
}

/// `\s+ \d{2}:\d{2}:\d{2}\.\d+ \s* (Z | [-+]\d\d:\d\d)`.
fn time_and_zone(rest: &str) -> bool {
    let t = rest.trim_start();
    if t.len() == rest.len() {
        return false;
    }
    let b = t.as_bytes();
    let shape = b"dd:dd:dd.";
    if b.len() < shape.len() + 1 {
        return false;
    }
    for (i, &c) in shape.iter().enumerate() {
        let ok = if c == b'd' {
            b[i].is_ascii_digit()
        } else {
            b[i] == c
        };
        if !ok {
            return false;
        }
    }
    let mut at = shape.len();
    let start = at;
    while at < b.len() && b[at].is_ascii_digit() {
        at += 1;
    }
    if at == start {
        return false;
    }
    let zone = t[at..].trim_start();
    zone == "Z"
        || (zone.len() == 6
            && matches!(zone.as_bytes()[0], b'+' | b'-')
            && zone.as_bytes()[1..3].iter().all(u8::is_ascii_digit)
            && zone.as_bytes()[3] == b':'
            && zone.as_bytes()[4..6].iter().all(u8::is_ascii_digit))
}

/// `TODAY`: the UTC day RubyGems was loaded on.
fn today() -> (i64, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    civil(secs.div_euclid(86_400))
}

/// `Gem.source_date_epoch`'s UTC day: `SOURCE_DATE_EPOCH`, else now.
fn source_date_epoch_day() -> (i64, u32, u32) {
    match std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
    {
        Some(secs) => civil(secs.div_euclid(86_400)),
        None => today(),
    }
}

/// The proleptic Gregorian date of a day count from 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// The YAML, read as Psych reads it
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Events(Vec<Event>);

impl EventReceiver for Events {
    fn on_event(&mut self, ev: Event) {
        self.0.push(ev);
    }
}

/// The first document, as Psych's `ToRuby` builds it.
fn load(text: &str) -> Result<Value, String> {
    let mut events = Events::default();
    Parser::new_from_str(text)
        .load(&mut events, false)
        .map_err(|e| format!("the gemspec's YAML does not parse: {e}"))?;
    let mut builder = Builder {
        events: events.0,
        at: 0,
        anchors: Vec::new(),
    };
    while let Some(ev) = builder.events.get(builder.at) {
        match ev {
            Event::StreamStart | Event::DocumentStart | Event::Nothing => builder.at += 1,
            Event::StreamEnd | Event::DocumentEnd => {
                return Err("the gemspec's YAML is empty".to_owned());
            }
            _ => return builder.node(),
        }
    }
    Err("the gemspec's YAML is empty".to_owned())
}

struct Builder {
    events: Vec<Event>,
    at: usize,
    anchors: Vec<(usize, Value)>,
}

/// A tag as Psych sees it: `!foo` stays `!foo`; `!!foo` is
/// `tag:yaml.org,2002:foo`.
fn tag_name(tag: &Tag) -> String {
    if tag.handle == "!!" {
        format!("tag:yaml.org,2002:{}", tag.suffix)
    } else if tag.handle.is_empty() {
        tag.suffix.clone()
    } else {
        format!("{}{}", tag.handle, tag.suffix)
    }
}

/// The class a `!ruby/object:` tag names, as `Gem::SafeYAML` permits them.
fn object_class(tag: &str) -> Result<Option<String>, String> {
    let Some(class) = tag.strip_prefix("!ruby/object:") else {
        return Ok(None);
    };
    match class {
        "Gem::Specification"
        | "Gem::Version"
        | "Gem::Requirement"
        | "Gem::Version::Requirement"
        | "Gem::Dependency"
        | "Gem::Platform" => Ok(Some(class.to_owned())),
        other => Err(format!("Tried to load unspecified class: {other}")),
    }
}

impl Builder {
    fn next(&mut self) -> Result<Event, String> {
        let ev = self
            .events
            .get(self.at)
            .cloned()
            .ok_or_else(|| "the gemspec's YAML ends early".to_owned())?;
        self.at += 1;
        Ok(ev)
    }

    fn anchor(&mut self, id: usize, value: &Value) {
        if id > 0 {
            self.anchors.push((id, value.clone()));
        }
    }

    fn node(&mut self) -> Result<Value, String> {
        match self.next()? {
            Event::Alias(id) => self
                .anchors
                .iter()
                .rev()
                .find(|(a, _)| *a == id)
                .map(|(_, v)| v.clone())
                .ok_or_else(|| "the gemspec's YAML has an unknown alias".to_owned()),
            Event::Scalar(text, style, anchor, tag) => {
                let value = scalar(&text, style, tag.as_ref())?;
                self.anchor(anchor, &value);
                Ok(value)
            }
            Event::SequenceStart(anchor, _tag) => {
                let mut items = Vec::new();
                loop {
                    if matches!(self.events.get(self.at), Some(Event::SequenceEnd)) {
                        self.at += 1;
                        break;
                    }
                    items.push(self.node()?);
                }
                let value = Value::Seq(items);
                self.anchor(anchor, &value);
                Ok(value)
            }
            Event::MappingStart(anchor, tag) => {
                let class = match &tag {
                    Some(tag) => object_class(&tag_name(tag))?,
                    None => None,
                };
                let mut entries = Vec::new();
                loop {
                    if matches!(self.events.get(self.at), Some(Event::MappingEnd)) {
                        self.at += 1;
                        break;
                    }
                    let key = self.node()?;
                    let value = self.node()?;
                    entries.push((key, value));
                }
                let value = Value::Map { class, entries };
                self.anchor(anchor, &value);
                Ok(value)
            }
            other => Err(format!("unexpected YAML event {other:?}")),
        }
    }
}

/// Psych's `deserialize` of a scalar: a tag says what it is, a quoted or
/// block scalar is a String, and a plain one is typed by `tokenize`.
fn scalar(text: &str, style: TScalarStyle, tag: Option<&Tag>) -> Result<Value, String> {
    if let Some(tag) = tag {
        let tag = tag_name(tag);
        match tag.as_str() {
            "!binary" | "tag:yaml.org,2002:binary" => {
                use base64::Engine as _;
                let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                return base64::engine::general_purpose::STANDARD
                    .decode(compact)
                    .map(Value::Bytes)
                    .map_err(|e| format!("a !binary scalar that is not base64: {e}"));
            }
            "!str" | "tag:yaml.org,2002:str" | "!ruby/string" => {
                return Ok(Value::Str(text.to_owned()));
            }
            "!ruby/symbol" | "!ruby/sym" => return permitted(Value::Sym(text.to_owned())),
            _ if tag.starts_with("!ruby/object:") => {
                object_class(&tag)?;
            }
            _ => {}
        }
    }
    if style != TScalarStyle::Plain {
        return Ok(Value::Str(text.to_owned()));
    }
    permitted(tokenize(text))
}

/// `Gem::SafeYAML::PERMITTED_SYMBOLS`: a dependency's type is the only
/// Symbol a gemspec holds.
fn permitted(value: Value) -> Result<Value, String> {
    match &value {
        Value::Sym(s) if s != "development" && s != "runtime" => {
            Err(format!("Tried to load unspecified class: Symbol (:{s})"))
        }
        _ => Ok(value),
    }
}

/// `Psych::ScalarScanner#tokenize`, for what a gemspec can hold.
fn tokenize(s: &str) -> Value {
    if s.is_empty() {
        return Value::Nil;
    }
    if looks_like_a_string(s) || s.contains('\n') {
        if s.chars().count() > 5 {
            return Value::Str(s.to_owned());
        }
        let lower = s.to_ascii_lowercase();
        let first = lower.as_bytes()[0];
        if !b"ytonf~".contains(&first) {
            return Value::Str(s.to_owned());
        }
        return match lower.as_str() {
            "~" | "null" => Value::Nil,
            "yes" | "true" | "on" => Value::Bool(true),
            "no" | "false" | "off" => Value::Bool(false),
            _ => Value::Str(s.to_owned()),
        };
    }
    if let Some(day) = timestamp(s) {
        return day;
    }
    if let Some(day) = date(s) {
        return day;
    }
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        ".inf" | "+.inf" => return Value::Float(f64::INFINITY),
        "-.inf" => return Value::Float(f64::NEG_INFINITY),
        ".nan" => return Value::Float(f64::NAN),
        _ => {}
    }
    if s.len() > 1 && s.starts_with(':') {
        let rest = &s[1..];
        for q in ['"', '\''] {
            if let Some(inner) = rest.strip_prefix(q)
                && let Some(end) = inner.rfind(q)
            {
                let name = &inner[..end];
                return Value::Sym(name.strip_prefix(':').unwrap_or(name).to_owned());
            }
        }
        return Value::Sym(rest.to_owned());
    }
    if let Some(i) = sexagesimal(s) {
        return Value::Int(i);
    }
    if let Some(f) = float(s) {
        return f;
    }
    if let Some(i) = integer(s) {
        return Value::Int(i);
    }
    Value::Str(s.to_owned())
}

/// `%r{^[^\d.:-]?[[:alpha:]_\s!@#$%\^&*(){}<>|/\\~;=]+}` at the start.
fn looks_like_a_string(s: &str) -> bool {
    let class = |c: char| {
        c.is_alphabetic() || c == '_' || c.is_whitespace() || "!@#$%^&*(){}<>|/\\~;=".contains(c)
    };
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if class(first) {
        return true;
    }
    !(first.is_ascii_digit() || ".:-".contains(first)) && chars.next().is_some_and(class)
}

/// Psych's `TIME`, read for its day: in the zone it names, which is the day
/// written. ponytail: a time with no zone is read by Psych in the local zone,
/// whose day can differ from the one written; this keeps the written day.
/// Gems are built with `Z`. Upgrade: the local offset, if one ever shows up.
fn timestamp(s: &str) -> Option<Value> {
    let (date_part, rest) = split_date(s)?;
    let rest = rest
        .strip_prefix(['T', 't'])
        .map(str::to_owned)
        .or_else(|| {
            let trimmed = rest.trim_start_matches([' ', '\t']);
            (trimmed.len() < rest.len()).then(|| trimmed.to_owned())
        })?;
    let b = rest.as_bytes();
    let mut at = 0;
    let digits = |at: &mut usize, min: usize, max: usize| {
        let start = *at;
        while *at < b.len() && b[*at].is_ascii_digit() && *at - start < max {
            *at += 1;
        }
        *at - start >= min
    };
    if !digits(&mut at, 1, 2) || b.get(at) != Some(&b':') {
        return None;
    }
    at += 1;
    if !digits(&mut at, 2, 2) || b.get(at) != Some(&b':') {
        return None;
    }
    at += 1;
    if !digits(&mut at, 2, 2) {
        return None;
    }
    if b.get(at) == Some(&b'.') {
        at += 1;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
        }
    }
    let zone = rest[at..].trim_start_matches([' ', '\t']);
    let zone_ok = zone.is_empty() || zone == "Z" || {
        let z = zone.as_bytes();
        let mut i = 1;
        let sign = matches!(z.first(), Some(b'+' | b'-'));
        let start = i;
        while i < z.len() && z[i].is_ascii_digit() && i - start < 2 {
            i += 1;
        }
        let hours = i > start;
        if z.get(i) == Some(&b':') {
            i += 1;
        }
        let mstart = i;
        while i < z.len() && z[i].is_ascii_digit() && i - mstart < 2 {
            i += 1;
        }
        sign && hours && (i == mstart || i - mstart == 2) && i == z.len()
    };
    if !zone_ok {
        return None;
    }
    Some(date_part)
}

/// `^-?\d{4}-\d{1,2}-\d{1,2}` and what follows it.
fn split_date(s: &str) -> Option<(Value, &str)> {
    let (negative, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let b = body.as_bytes();
    if b.len() < 8 || !b[..4].iter().all(u8::is_ascii_digit) || b[4] != b'-' {
        return None;
    }
    let mut at = 5;
    let mstart = at;
    while at < b.len() && b[at].is_ascii_digit() && at - mstart < 2 {
        at += 1;
    }
    if at == mstart || b.get(at) != Some(&b'-') {
        return None;
    }
    let month: u32 = body[mstart..at].parse().ok()?;
    at += 1;
    let dstart = at;
    while at < b.len() && b[at].is_ascii_digit() && at - dstart < 2 {
        at += 1;
    }
    if at == dstart {
        return None;
    }
    let day: u32 = body[dstart..at].parse().ok()?;
    let mut year: i64 = body[..4].parse().ok()?;
    if negative {
        year = -year;
    }
    Some((Value::Day(year, month, day), &body[at..]))
}

/// A date alone, `/^\d{4}-(?:1[012]|0\d|\d)-(?:[12]\d|3[01]|0\d|\d)$/`, that
/// `Date.strptime` reads.
fn date(s: &str) -> Option<Value> {
    if s.starts_with('-') {
        return None;
    }
    let (Value::Day(y, m, d), rest) = split_date(s)? else {
        return None;
    };
    if !rest.is_empty() || !(1..=12).contains(&m) || d == 0 {
        // `0\d` months and days pass the pattern but not `strptime`.
        return (rest.is_empty() && m <= 12 && d <= 31).then(|| Value::Str(s.to_owned()));
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if d > days[(m - 1) as usize] {
        return Some(Value::Str(s.to_owned()));
    }
    Some(Value::Day(y, m, d))
}

/// `/^[-+]?[0-9][0-9_]*(:[0-5]?[0-9]){1,2}$/`, base 60.
fn sexagesimal(s: &str) -> Option<i128> {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let mut parts = body.split(':');
    let head = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() || rest.len() > 2 {
        return None;
    }
    if !head.starts_with(|c: char| c.is_ascii_digit())
        || !head.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    for part in &rest {
        let ok = match part.as_bytes() {
            [a] => a.is_ascii_digit(),
            [a, b] => (b'0'..=b'5').contains(a) && b.is_ascii_digit(),
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    // `n.to_i * 60 ** (e - 2).abs`, over `s.split(':')` with its sign.
    let all: Vec<&str> = s.split(':').collect();
    let mut total: i128 = 0;
    for (e, n) in all.iter().enumerate() {
        let n: i128 = ruby_to_i(n);
        total += n * 60i128.pow((e as i32 - 2).unsigned_abs());
    }
    Some(total)
}

/// `String#to_i`: leading digits (and underscores between them), signed.
fn ruby_to_i(s: &str) -> i128 {
    let (negative, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits: String = body
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .filter(char::is_ascii_digit)
        .collect();
    let n = digits.parse::<i128>().unwrap_or(0);
    if negative { -n } else { n }
}

/// Psych's `FLOAT`: `[-+]?([0-9][0-9_,]*)?\.[0-9]*([eE][-+][0-9]+)?`.
fn float(s: &str) -> Option<Value> {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let (int, frac) = body.split_once('.')?;
    if !int.is_empty()
        && !(int.starts_with(|c: char| c.is_ascii_digit())
            && int
                .chars()
                .all(|c| c.is_ascii_digit() || c == '_' || c == ','))
    {
        return None;
    }
    let (digits, exponent) = match frac.find(['e', 'E']) {
        Some(at) => (&frac[..at], Some(&frac[at + 1..])),
        None => (frac, None),
    };
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if let Some(exp) = exponent {
        let e = exp.strip_prefix(['-', '+'])?;
        if e.is_empty() || !e.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
    }
    if int.is_empty() && digits.is_empty() && exponent.is_none() {
        // `/\A[-+]?\.\Z/` stays a String.
        return Some(Value::Str(s.to_owned()));
    }
    let cleaned: String = s.chars().filter(|c| *c != ',' && *c != '_').collect();
    let cleaned = cleaned.replacen(".e", "e", 1).replacen(".E", "E", 1);
    let cleaned = cleaned.strip_suffix('.').unwrap_or(&cleaned).to_owned();
    cleaned.parse::<f64>().ok().map(Value::Float)
}

/// Psych's `INTEGER_LEGACY`, read by `parse_int`: binary, octal (a leading
/// `0`), decimal and hex, with `_` and `,` dropped.
fn integer(s: &str) -> Option<i128> {
    let (negative, body) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let clean = |t: &str| -> String { t.chars().filter(|c| *c != '_' && *c != ',').collect() };
    let n = if let Some(bin) = body.strip_prefix("0b") {
        if bin.is_empty() || !bin.chars().all(|c| matches!(c, '0' | '1' | '_' | ',')) {
            return None;
        }
        i128::from_str_radix(&clean(bin), 2).ok()?
    } else if let Some(hex) = body.strip_prefix("0x") {
        if hex.is_empty()
            || !hex
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '_' || c == ',')
        {
            return None;
        }
        i128::from_str_radix(&clean(hex), 16).ok()?
    } else if body.len() > 1 && body.starts_with('0') {
        if !body[1..]
            .chars()
            .all(|c| matches!(c, '0'..='7' | '_' | ','))
        {
            return None;
        }
        i128::from_str_radix(&clean(&body[1..]), 8).ok()?
    } else {
        let b = body.as_bytes();
        if body == "0" {
            0
        } else {
            if b.is_empty() || !(b'1'..=b'9').contains(&b[0]) {
                return None;
            }
            // `[1-9](?:[0-9]|,[0-9]|_[0-9])*`
            let mut i = 1;
            while i < b.len() {
                if b[i].is_ascii_digit() {
                    i += 1;
                } else if matches!(b[i], b',' | b'_')
                    && b.get(i + 1).is_some_and(u8::is_ascii_digit)
                {
                    i += 2;
                } else {
                    return None;
                }
            }
            clean(body).parse().ok()?
        }
    };
    Some(if negative { -n } else { n })
}

#[cfg(test)]
mod tests;

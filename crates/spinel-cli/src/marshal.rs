//! A reader for Ruby's Marshal format 4.8, enough of it for a legacy gem
//! index (#89): `specs.4.8.gz`, a Marshal of `[name, Gem::Version, platform]`
//! triples, and `quick/Marshal.4.8/<full name>.gemspec.rz`, a deflated
//! Marshal of a `Gem::Specification`. Every type `Marshal.dump` writes is
//! read; objects come back as their class and instance variables, user
//! dumps (`_dump`, `marshal_dump`) as their class and data, never run.

use std::fmt;

/// A value as Marshal wrote it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    /// A Float or a Bignum, kept as Marshal writes it.
    Number(String),
    /// A String's bytes; its encoding is dropped.
    Str(Vec<u8>),
    Symbol(String),
    Array(Vec<Value>),
    Hash(Vec<(Value, Value)>),
    /// `o`: a plain object's class and instance variables (`@name`).
    Object(String, Vec<(String, Value)>),
    /// `u`: what a class's `_dump` returned.
    UserDump(String, Vec<u8>),
    /// `U`: what a class's `marshal_dump` returned.
    UserMarshal(String, Box<Value>),
    /// A class, module, regexp or struct, whose content nothing here reads.
    Other,
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(bytes) => std::str::from_utf8(bytes).ok(),
            Value::Symbol(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// An object's instance variable, `@name` given as `name`.
    pub fn ivar(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Object(_, ivars) => ivars
                .iter()
                .find(|(k, _)| k.strip_prefix('@') == Some(name))
                .map(|(_, v)| v),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not a Marshal 4.8 dump: {}", self.0)
    }
}

impl std::error::Error for Error {}

/// Read one Marshal dump.
pub fn load(data: &[u8]) -> Result<Value, Error> {
    if data.len() < 2 || data[0] != 4 || data[1] > 8 {
        return Err(Error("the version is not 4.8".to_owned()));
    }
    let mut reader = Reader {
        data,
        at: 2,
        symbols: Vec::new(),
        objects: Vec::new(),
        depth: 0,
    };
    reader.value()
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    symbols: Vec<String>,
    /// Every object read, for `@` links; `None` while it is being read.
    objects: Vec<Option<Value>>,
    depth: usize,
}

/// Deeper than this, a dump is taken to be hostile.
const MAX_DEPTH: usize = 256;

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, Error> {
        let b = *self
            .data
            .get(self.at)
            .ok_or_else(|| Error("it ends early".to_owned()))?;
        self.at += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| Error("it ends early".to_owned()))?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }

    fn int(&mut self) -> Result<i64, Error> {
        let c = self.byte()? as i8;
        Ok(match c {
            0 => 0,
            5..=127 => i64::from(c) - 5,
            -128..=-5 => i64::from(c) + 5,
            1..=4 => {
                let mut n = 0i64;
                for i in 0..c {
                    n |= i64::from(self.byte()?) << (8 * i);
                }
                n
            }
            _ => {
                let len = -c;
                let mut n = -1i64;
                for i in 0..len {
                    n &= !(0xff << (8 * i));
                    n |= i64::from(self.byte()?) << (8 * i);
                }
                n
            }
        })
    }

    fn len(&mut self) -> Result<usize, Error> {
        let n = self.int()?;
        usize::try_from(n)
            .ok()
            .filter(|&n| n <= self.data.len())
            .ok_or_else(|| Error(format!("a length of {n}")))
    }

    fn raw_string(&mut self) -> Result<Vec<u8>, Error> {
        let n = self.len()?;
        Ok(self.bytes(n)?.to_vec())
    }

    fn symbol(&mut self) -> Result<String, Error> {
        match self.byte()? {
            b':' => {
                let s = String::from_utf8_lossy(&self.raw_string()?).into_owned();
                self.symbols.push(s.clone());
                Ok(s)
            }
            b';' => {
                let i = self.len()?;
                self.symbols
                    .get(i)
                    .cloned()
                    .ok_or_else(|| Error(format!("symbol link {i}")))
            }
            b'I' => {
                // A symbol with an encoding.
                let s = self.symbol()?;
                self.ivars()?;
                Ok(s)
            }
            t => Err(Error(format!("a symbol of type {:?}", t as char))),
        }
    }

    fn ivars(&mut self) -> Result<Vec<(String, Value)>, Error> {
        let n = self.len()?;
        let mut ivars = Vec::new();
        for _ in 0..n {
            let key = self.symbol()?;
            let value = self.value()?;
            ivars.push((key, value));
        }
        Ok(ivars)
    }

    /// Reserve the object's slot, read it, and fill the slot.
    fn object(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<Value, Error>,
    ) -> Result<Value, Error> {
        let slot = self.objects.len();
        self.objects.push(None);
        let value = read(self)?;
        self.objects[slot] = Some(value.clone());
        Ok(value)
    }

    fn value(&mut self) -> Result<Value, Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error("it nests too deep".to_owned()));
        }
        let value = self.value_inner();
        self.depth -= 1;
        value
    }

    fn value_inner(&mut self) -> Result<Value, Error> {
        let t = self.byte()?;
        match t {
            b'0' => Ok(Value::Nil),
            b'T' => Ok(Value::Bool(true)),
            b'F' => Ok(Value::Bool(false)),
            b'i' => Ok(Value::Int(self.int()?)),
            b':' | b';' => {
                self.at -= 1;
                Ok(Value::Symbol(self.symbol()?))
            }
            b'@' => {
                let i = self.len()?;
                match self.objects.get(i) {
                    Some(Some(v)) => Ok(v.clone()),
                    // A link to an object still being read: a cycle.
                    Some(None) => Ok(Value::Nil),
                    None => Err(Error(format!("object link {i}"))),
                }
            }
            b'I' => {
                let value = self.value()?;
                self.ivars()?;
                Ok(value)
            }
            b'e' => {
                self.symbol()?;
                self.value()
            }
            b'"' => self.object(|r| Ok(Value::Str(r.raw_string()?))),
            b'f' => self.object(|r| {
                Ok(Value::Number(
                    String::from_utf8_lossy(&r.raw_string()?).into_owned(),
                ))
            }),
            b'l' => self.object(|r| {
                let sign = r.byte()?;
                let n = r.len()?;
                let bytes = r.bytes(n.saturating_mul(2))?;
                let hex: String = bytes.iter().rev().map(|b| format!("{b:02x}")).collect();
                Ok(Value::Number(format!(
                    "{}0x{hex}",
                    if sign == b'-' { "-" } else { "" }
                )))
            }),
            b'[' => self.object(|r| {
                let n = r.len()?;
                let mut items = Vec::new();
                for _ in 0..n {
                    items.push(r.value()?);
                }
                Ok(Value::Array(items))
            }),
            b'{' | b'}' => self.object(|r| {
                let n = r.len()?;
                let mut pairs = Vec::new();
                for _ in 0..n {
                    let k = r.value()?;
                    let v = r.value()?;
                    pairs.push((k, v));
                }
                if t == b'}' {
                    r.value()?;
                }
                Ok(Value::Hash(pairs))
            }),
            b'o' => self.object(|r| {
                let class = r.symbol()?;
                let ivars = r.ivars()?;
                Ok(Value::Object(class, ivars))
            }),
            b'u' => self.object(|r| {
                let class = r.symbol()?;
                Ok(Value::UserDump(class, r.raw_string()?))
            }),
            b'U' => self.object(|r| {
                let class = r.symbol()?;
                Ok(Value::UserMarshal(class, Box::new(r.value()?)))
            }),
            b'C' => {
                self.symbol()?;
                self.value()
            }
            b'S' => self.object(|r| {
                r.symbol()?;
                r.ivars()?;
                Ok(Value::Other)
            }),
            b'/' => self.object(|r| {
                r.raw_string()?;
                r.byte()?;
                Ok(Value::Other)
            }),
            b'c' | b'm' | b'M' => self.object(|r| {
                r.raw_string()?;
                Ok(Value::Other)
            }),
            t => Err(Error(format!("type {:?}", t as char))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_ruby_dumps() {
        // Marshal.dump([nil, true, 0, 300, -300, "é", :a, :a, {"k" => 1.5}])
        let data = b"\x04\x08[\x0e0Ti\x00i\x02,\x01i\xfe\xd4\xfeI\"\x07\xc3\xa9\x06:\x06ET:\x06a;\x06{\x06I\"\x06k\x06;\x00Tf\x081.5";
        let v = load(data).unwrap();
        let items = v.as_array().unwrap();
        assert_eq!(items[0], Value::Nil);
        assert_eq!(items[1], Value::Bool(true));
        assert_eq!(items[2], Value::Int(0));
        assert_eq!(items[3], Value::Int(300));
        assert_eq!(items[4], Value::Int(-300));
        assert_eq!(items[5].as_str(), Some("é"));
        assert_eq!(items[6], Value::Symbol("a".to_owned()));
        assert_eq!(items[7], Value::Symbol("a".to_owned()));
        assert_eq!(
            items[8],
            Value::Hash(vec![(
                Value::Str(b"k".to_vec()),
                Value::Number("1.5".to_owned())
            )])
        );
    }

    #[test]
    fn links_and_truncation() {
        // a = "x"; Marshal.dump([a, a])
        let v = load(b"\x04\x08[\x07I\"\x06x\x06:\x06ET@\x06").unwrap();
        assert_eq!(v.as_array().unwrap()[1].as_str(), Some("x"));
        assert!(load(b"\x04\x08[\x07I\"\x06x").is_err());
        assert!(load(b"\x04\x09[\x00").is_err());
    }
}

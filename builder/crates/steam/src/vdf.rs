//! Steam's binary VDF, the layout of its own `shortcuts.vdf`: a map's items one after another, each a type byte, a
//! NUL-ended name and its value (0 a map, ended by a lone 0x08; 1 a NUL-ended UTF-8 string; 2 a little-endian i32; 3 a
//! little-endian f32), the root ended by its own 0x08. Read and written back byte for byte (the Longshot Launcher
//! checked its reader so against the file Steam wrote on a Steam Deck), the items in their order.
//!
//! A file that rewrites the library must never lose what it can't read: one that stops before its last 0x08 (a write
//! cut short) or holds another type is refused ([`Error::Unreadable`]), never read leniently and written back short.

use thiserror::Error;

/// The byte that ends a map.
const END: u8 = 0x08;

/// A value in a map.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A map of its own (type 0).
    Map(Map),
    /// A string (type 1).
    Str(String),
    /// A 32-bit integer (type 2).
    Int(i32),
    /// A 32-bit float (type 3).
    Float(f32),
}

/// A map: its items, named, in the file's order (names may repeat in a file someone else wrote: kept as they are).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map(pub Vec<(String, Value)>);

/// Why a file is not one to rewrite.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// It stops partway, or holds what this reader can't write back as it was.
    #[error("{0}")]
    Unreadable(String),
}

impl Map {
    /// The value named `key` (the first of that name).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The string named `key`, or "".
    pub fn str(&self, key: &str) -> &str {
        match self.get(key) {
            Some(Value::Str(s)) => s,
            _ => "",
        }
    }

    /// The integer named `key`, or 0.
    pub fn int(&self, key: &str) -> i32 {
        match self.get(key) {
            Some(Value::Int(n)) => *n,
            _ => 0,
        }
    }

    /// The value named `key` set: in its place when there is one, else added at the end.
    pub fn set(&mut self, key: &str, value: Value) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(item) => item.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    /// The map named `key`, added empty at the end when there is none.
    pub fn map_mut(&mut self, key: &str) -> &mut Map {
        let at = match self.0.iter().position(|(k, v)| k == key && matches!(v, Value::Map(_))) {
            Some(i) => i,
            None => {
                self.0.push((key.to_string(), Value::Map(Map::default())));
                self.0.len() - 1
            }
        };
        match &mut self.0[at].1 {
            Value::Map(m) => m,
            _ => unreachable!("found as a map, or just added as one"),
        }
    }
}

/// `bytes` read as a file's root map. No bytes: an empty map (no library yet).
pub fn parse(bytes: &[u8]) -> Result<Map, Error> {
    if bytes.is_empty() {
        return Ok(Map::default());
    }
    let mut at = 0;
    let root = body(bytes, &mut at)?;
    // (bytes past the root's end: a stray 0x08 closed it early, and the entries after it would be lost)
    if at != bytes.len() {
        return Err(not_a_library());
    }
    Ok(root)
}

/// The error of a file that does not read as a library.
pub fn not_a_library() -> Error {
    Error::Unreadable(
        "the Steam library file does not read as a library (its entries are not where the file says they are). \
         Nothing was changed: rewriting it now would lose the shortcuts it holds. Starting Steam again usually \
         repairs it."
            .into(),
    )
}

fn cut() -> Error {
    Error::Unreadable(
        "the Steam library file stops partway through an entry (a write was cut short at some point). Nothing was \
         changed: rewriting it now would lose the entries the cut hides. Starting Steam again usually repairs it."
            .into(),
    )
}

/// A map's items from `at`, up to and past its 0x08.
fn body(b: &[u8], at: &mut usize) -> Result<Map, Error> {
    let mut map = Map::default();
    loop {
        let Some(&kind) = b.get(*at) else { return Err(cut()) };
        *at += 1;
        if kind == END {
            return Ok(map);
        }
        let name = string(b, at)?;
        let value = match kind {
            0 => Value::Map(body(b, at)?),
            1 => Value::Str(string(b, at)?),
            2 => Value::Int(i32::from_le_bytes(four(b, at)?)),
            3 => Value::Float(f32::from_le_bytes(four(b, at)?)),
            other => {
                return Err(Error::Unreadable(format!(
                    "the Steam library file holds a value of type {other} at byte {}, which this can't write back as \
                     it is: nothing was changed.",
                    *at - 1
                )));
            }
        };
        map.0.push((name, value));
    }
}

fn string(b: &[u8], at: &mut usize) -> Result<String, Error> {
    let rest = b.get(*at..).ok_or_else(cut)?;
    let end = rest.iter().position(|&c| c == 0).ok_or_else(cut)?;
    let s = String::from_utf8_lossy(&rest[..end]).into_owned();
    *at += end + 1;
    Ok(s)
}

fn four(b: &[u8], at: &mut usize) -> Result<[u8; 4], Error> {
    let v: [u8; 4] = b.get(*at..*at + 4).ok_or_else(cut)?.try_into().map_err(|_| cut())?;
    *at += 4;
    Ok(v)
}

/// `root` as a file's bytes.
pub fn dump(root: &Map) -> Vec<u8> {
    let mut out = Vec::new();
    items(root, &mut out);
    out
}

fn items(map: &Map, out: &mut Vec<u8>) {
    for (name, value) in &map.0 {
        let kind = match value {
            Value::Map(_) => 0,
            Value::Str(_) => 1,
            Value::Int(_) => 2,
            Value::Float(_) => 3,
        };
        out.push(kind);
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        match value {
            Value::Map(m) => items(m, out),
            Value::Str(s) => {
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Value::Int(n) => out.extend_from_slice(&n.to_le_bytes()),
            Value::Float(f) => out.extend_from_slice(&f.to_le_bytes()),
        }
    }
    out.push(END);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library as Steam writes it: one shortcut with every kind of value.
    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"\x00shortcuts\x00\x000\x00");
        b.extend_from_slice(b"\x02appid\x00");
        b.extend_from_slice(&(-123456789i32).to_le_bytes());
        b.extend_from_slice(b"\x01AppName\x00Emu\x00\x01Exe\x00\"/opt/emu\"\x00");
        b.extend_from_slice(b"\x03Ratio\x00");
        b.extend_from_slice(&1.5f32.to_le_bytes());
        b.extend_from_slice(b"\x00tags\x00\x010\x00favorite\x00\x08");
        b.extend_from_slice(b"\x08\x08\x08");
        b
    }

    #[test]
    fn a_library_reads_and_writes_back_byte_for_byte() {
        let b = sample();
        let root = parse(&b).unwrap();
        assert_eq!(dump(&root), b);
        let Some(Value::Map(shortcuts)) = root.get("shortcuts") else { panic!("{root:?}") };
        let Some(Value::Map(e)) = shortcuts.get("0") else { panic!() };
        assert_eq!((e.int("appid"), e.str("AppName"), e.str("Exe")), (-123456789, "Emu", "\"/opt/emu\""));
        assert_eq!(e.get("Ratio"), Some(&Value::Float(1.5)));
        assert_eq!(parse(&[]).unwrap(), Map::default(), "no file yet: an empty library");
    }

    #[test]
    fn a_file_cut_short_or_of_another_type_is_refused() {
        let b = sample();
        for len in [1, 10, b.len() - 1] {
            assert!(matches!(parse(&b[..len]), Err(Error::Unreadable(t)) if t.contains("partway")), "{len}");
        }
        let mut odd = b"\x00shortcuts\x00".to_vec();
        odd.extend_from_slice(b"\x05x\x00\x00\x08\x08");
        assert!(matches!(parse(&odd), Err(Error::Unreadable(t)) if t.contains("type 5")));
        // (a stray 0x08 first: an empty library with the real one unread after it)
        let stray = [&[END][..], &b].concat();
        assert_eq!(parse(&stray), Err(not_a_library()));
    }

    #[test]
    fn set_and_map_mut_keep_the_order() {
        let mut m = Map::default();
        m.set("a", Value::Int(1));
        m.set("b", Value::Str("x".into()));
        m.set("a", Value::Int(2));
        assert_eq!(m.0.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(m.int("a"), 2);
        m.map_mut("tags").set("0", Value::Str("t".into()));
        assert_eq!(m.map_mut("tags").str("0"), "t");
        assert_eq!(m.0.len(), 3);
    }
}

//! `profile.json` v1: one JSON object, its keys in the file's order.
//!
//! ```json
//! { "v": 1, "name": "Ana", "color": "#7CC242", "version": "v4.0.0",
//!   "start_slot": "slot_02", "last_played": "2026-11-24T21:04:00.0000000Z", "copy_of": "default",
//!   "game": "/games/oot.nup", "upscalers": ["dlss", "xess"], "licenses": {"nvidia": "<the pin accepted>"} }
//! ```
//!
//! Every key is optional; a file without `"v"` is read as v1 (the launcher's rule too). Each value is kept as the
//! file's text: a key this crate does not know (the launcher's, the Mod API's `"mods"`, a newer one's) is written back
//! as it was, in its place, and so is a key it knows and nothing changed. A key set that was not there goes after the
//! others.
//!
//! Read strictly: one JSON object in UTF-8 (a byte order mark before it allowed), no key twice, `"v"` 1 (a newer one
//! is refused, so it is never written over; none is 1). The other known keys are read when they are of their type and
//! are otherwise kept as they are; a key this crate does not know is never checked.

use std::fmt;
use std::io::Write as _;
use std::path::Path;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

/// The format's version.
pub const VERSION: u64 = 1;

/// The most bytes of a `profile.json` (a thousand mods are about 60 KB).
pub const FILE_MAX: u64 = 1 << 20;

/// Why a `profile.json` cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// There is no file.
    Missing,
    /// It cannot be read.
    Io(String),
    /// It is [`FILE_MAX`] bytes or more.
    TooLarge,
    /// It is not JSON, or not one object.
    Json(String),
    /// A key twice.
    Twice(String),
    /// A newer format than this one: never written over.
    Newer(u64),
    /// JSON, and not of the format: what is wrong.
    Shape(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Missing => f.write_str("there is no such file"),
            Error::Io(e) => write!(f, "it cannot be read ({e})"),
            Error::TooLarge => f.write_str("it is too large"),
            Error::Json(e) => write!(f, "it is not a JSON object ({e})"),
            Error::Twice(k) => write!(f, "{k:?} twice"),
            Error::Newer(n) => write!(f, "it is a newer profile (\"v\": {n}) than this one reads"),
            Error::Shape(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

/// An upscaler a profile's build includes (the builder's `--upscaler` names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Upscaler {
    /// NVIDIA DLSS.
    Dlss,
    /// AMD FSR 4.
    Fsr4,
    /// Intel XeSS.
    Xess,
}

impl Upscaler {
    /// Every one, in the file's order of names.
    pub const ALL: [Upscaler; 3] = [Upscaler::Dlss, Upscaler::Fsr4, Upscaler::Xess];

    /// Its name in the file.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Upscaler::Dlss => "dlss",
            Upscaler::Fsr4 => "fsr4",
            Upscaler::Xess => "xess",
        }
    }

    /// The upscaler of a name, `None` for one this crate does not know.
    #[must_use]
    pub fn of_name(name: &str) -> Option<Upscaler> {
        Upscaler::ALL.into_iter().find(|u| u.name() == name)
    }
}

/// A `profile.json`: its keys and their values' text, in the file's order.
#[derive(Clone, Debug)]
pub struct Profile {
    members: Vec<(String, Box<RawValue>)>,
}

/// The same keys in the same order, each value the same text.
impl PartialEq for Profile {
    fn eq(&self, other: &Profile) -> bool {
        self.members.len() == other.members.len()
            && self.members.iter().zip(&other.members).all(|((k, v), (o, w))| k == o && v.get() == w.get())
    }
}

impl Eq for Profile {}

/// An object's members in order (serde_json's own map is sorted).
struct Members(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Members {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Members;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Members, A::Error> {
                let mut out = Vec::new();
                while let Some(entry) = map.next_entry::<String, Box<RawValue>>()? {
                    out.push(entry);
                }
                Ok(Members(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// The members of the object `text`, no key twice.
pub(crate) fn members(text: &str) -> Result<Vec<(String, Box<RawValue>)>, Error> {
    let Members(m) = serde_json::from_str(text).map_err(|e| Error::Json(e.to_string()))?;
    for (i, (k, _)) in m.iter().enumerate() {
        if m[..i].iter().any(|(o, _)| o == k) {
            return Err(Error::Twice(k.clone()));
        }
    }
    Ok(m)
}

/// The JSON text of a string.
pub(crate) fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Text this crate made, as a value (it is JSON by construction).
fn raw(text: String) -> Box<RawValue> {
    RawValue::from_string(text).unwrap_or_default()
}

/// A file's text, at most `max` bytes, a byte order mark before it left out.
pub(crate) fn read_text(path: &Path, max: u64) -> Result<String, Error> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::Missing),
        Err(e) => return Err(Error::Io(e.to_string())),
    };
    if bytes.len() as u64 >= max {
        return Err(Error::TooLarge);
    }
    let text = String::from_utf8(bytes).map_err(|_| Error::Json("it is not UTF-8".into()))?;
    Ok(text.strip_prefix('\u{feff}').map(str::to_owned).unwrap_or(text))
}

/// `bytes` written to a new file at `path` and flushed to the disk.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

impl Default for Profile {
    fn default() -> Profile {
        Profile::new()
    }
}

impl Profile {
    /// A new profile: `"v": 1` alone.
    #[must_use]
    pub fn new() -> Profile {
        Profile { members: vec![("v".into(), raw(VERSION.to_string()))] }
    }

    /// The profile of a file's text, or why it is not one.
    ///
    /// # Errors
    /// Text that is not a `profile.json` v1 (the module's rules).
    pub fn parse(text: &str) -> Result<Profile, Error> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let p = Profile { members: members(text)? };
        match p.raw("v").map(|v| serde_json::from_str::<u64>(v.get())) {
            // (none: read as v1, as the launcher does)
            None | Some(Ok(VERSION)) => {}
            Some(Ok(n)) if n > VERSION => return Err(Error::Newer(n)),
            Some(_) => return Err(Error::Shape("\"v\" is not 1".into())),
        }
        Ok(p)
    }

    /// The `profile.json` at `path`.
    ///
    /// # Errors
    /// [`Error::Missing`] when there is none; else a file that cannot be read, or is not a `profile.json` v1.
    pub fn read(path: &Path) -> Result<Profile, Error> {
        Profile::parse(&read_text(path, FILE_MAX)?)
    }

    /// The file's text: a key a line, each value as it is kept.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut t = String::from("{\n");
        for (i, (k, v)) in self.members.iter().enumerate() {
            t.push_str("  ");
            t.push_str(&json_string(k));
            t.push_str(": ");
            t.push_str(v.get());
            t.push_str(if i + 1 < self.members.len() { ",\n" } else { "\n" });
        }
        t.push_str("}\n");
        t
    }

    /// Written to `path` whole or not at all: `<path>.new` written and flushed, then moved over it.
    ///
    /// # Errors
    /// The file could not be written; `path` is then as it was.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".new");
        let tmp = std::path::PathBuf::from(tmp);
        let written = write_whole(&tmp, self.to_text().as_bytes()).and_then(|()| std::fs::rename(&tmp, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }

    /// The keys, in the file's order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|(k, _)| k.as_str())
    }

    /// A key's value as the file has it.
    #[must_use]
    pub fn raw(&self, key: &str) -> Option<&RawValue> {
        self.members.iter().find(|(k, _)| k == key).map(|(_, v)| &**v)
    }

    /// A key set to `value` (JSON, as it is to be written) in its place, or after the others when it is new: for a
    /// key of another's (the Mod API's `"mods"`) that this crate keeps without reading.
    pub fn put(&mut self, key: &str, value: Box<RawValue>) {
        match self.members.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.members.push((key.to_owned(), value)),
        }
    }

    /// A key taken away.
    pub fn remove(&mut self, key: &str) {
        self.members.retain(|(k, _)| k != key);
    }

    /// A key's value when it is a string.
    #[must_use]
    pub fn string(&self, key: &str) -> Option<String> {
        self.raw(key).and_then(|v| serde_json::from_str(v.get()).ok())
    }

    /// A key set to a string, or taken away (`None`). `"v"` and `"upscalers"` have setters of their own.
    pub fn set_string(&mut self, key: &str, value: Option<&str>) {
        debug_assert!(!matches!(key, "v" | "upscalers"), "{key} is not a string");
        match value {
            Some(s) => self.put(key, raw(json_string(s))),
            None => self.remove(key),
        }
    }

    /// `"name"`: the profile's name, as the launcher shows it.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        self.string("name")
    }

    /// `"color"`: its colour (`"#rrggbb"`).
    #[must_use]
    pub fn color(&self) -> Option<String> {
        self.string("color")
    }

    /// `"version"`: the Longshot version it plays (the launcher's name for it); none, the launcher's default.
    #[must_use]
    pub fn version(&self) -> Option<String> {
        self.string("version")
    }

    /// `"game"`: the game copy its builds are made from, a `.nup` or a dump folder.
    #[must_use]
    pub fn game(&self) -> Option<String> {
        self.string("game")
    }

    /// `"last_played"`: when it last played (ISO 8601), as written.
    #[must_use]
    pub fn last_played(&self) -> Option<String> {
        self.string("last_played")
    }

    /// `"copy_of"`: the profile it was copied from.
    #[must_use]
    pub fn copy_of(&self) -> Option<String> {
        self.string("copy_of")
    }

    /// `"start_slot"` as a slot number: the slot's folder (`"slot_02"`), or its number (`"2"`, `2`), 0 to 99. `None`
    /// when there is none; `Err` with why for a value that is no slot. The game reads it by the same rule
    /// (host/longshot_profile.h's `longshot_profile_start_slot`).
    #[must_use]
    pub fn start_slot(&self) -> Option<Result<u32, String>> {
        let v: serde_json::Value = serde_json::from_str(self.raw("start_slot")?.get()).ok()?;
        let digits = match &v {
            serde_json::Value::Null => return None,
            serde_json::Value::String(s) => {
                let s = s.trim();
                let lower = s.to_ascii_lowercase();
                lower.strip_prefix("slot_").map_or(s.to_owned(), str::to_owned)
            }
            serde_json::Value::Number(n) => n.to_string(),
            _ => return Some(Err(format!("{v} is not a slot"))),
        };
        Some(match digits.parse::<u32>() {
            Ok(n) if n <= 99 && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => Ok(n),
            _ => Err(format!("{v} is no slot (slot_00 to slot_99)")),
        })
    }

    /// `"upscalers"`' names, as written (strings only).
    #[must_use]
    pub fn upscaler_names(&self) -> Vec<String> {
        self.raw("upscalers")
            .and_then(|u| serde_json::from_str::<Vec<serde_json::Value>>(u.get()).ok())
            .map_or_else(Vec::new, |a| a.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
    }

    /// `"upscalers"`: the upscalers its build includes.
    ///
    /// # Errors
    /// A name this crate does not know (a newer launcher's): the build cannot be what the profile asks.
    pub fn upscalers(&self) -> Result<Vec<Upscaler>, String> {
        self.upscaler_names().iter().map(|n| Upscaler::of_name(n).ok_or_else(|| n.clone())).collect()
    }

    /// `"upscalers"` set (each once, in the order given), in its place.
    pub fn set_upscalers(&mut self, upscalers: &[Upscaler]) {
        let mut names: Vec<&str> = Vec::new();
        for u in upscalers {
            if !names.contains(&u.name()) {
                names.push(u.name());
            }
        }
        let text = format!("[{}]", names.iter().map(|n| json_string(n)).collect::<Vec<_>>().join(", "));
        self.put("upscalers", raw(text));
    }

    /// `"licenses"`: the vendors' licences the player accepted, each by its vendor's name (`"nvidia"`, `"amd"`,
    /// `"intel"`: the builder's `--accept-licenses` names) for the pin of the files it covers: `{"nvidia": "<pin>"}`.
    /// A value that is not text is left out (and kept in the file as it is).
    #[must_use]
    pub fn licenses(&self) -> std::collections::BTreeMap<String, String> {
        self.raw("licenses")
            .and_then(|l| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(l.get()).ok())
            .map(|m| m.into_iter().filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned()))).collect())
            .unwrap_or_default()
    }

    /// The licence of `vendor` accepted for `pin` in `"licenses"`, in its place: the other vendors' entries kept as
    /// they are (a newer builder's too). A `"licenses"` that is not an object is replaced.
    pub fn set_license(&mut self, vendor: &str, pin: &str) {
        let mut m = self
            .raw("licenses")
            .and_then(|l| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(l.get()).ok())
            .unwrap_or_default();
        m.insert(vendor.to_owned(), serde_json::Value::String(pin.to_owned()));
        let text = serde_json::to_string(&m).unwrap_or_else(|_| "{}".into());
        self.put("licenses", raw(text));
    }
}

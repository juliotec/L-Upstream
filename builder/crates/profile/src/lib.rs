//! The guest profile: how often the game's functions, their blocks and their conditional branches ran in play, for
//! the lifter to place and optimise the code by (branch weights, entry counts, cold functions, hot units at -O2).
//!
//! It replaces the old lifter's hot list and clang profile, which were keyed by the lifted C: any change to the lifter,
//! or another clang, made them stale. This one is keyed by the guest code: a function by its entry and a hash of its
//! instruction words ([`code_hash`]), a block by its first instruction's address. A function whose words changed (a
//! game update, a patch) no longer matches, and its counts are left out, never put on other code. Counts from several
//! runs add up ([`Profile::merge`]). The text holds addresses and counts only, nothing of the game's code.
//!
//! The text, one record a line (hex addresses and hashes, decimal counts):
//!
//! ```text
//! longshot-guest-profile 1 build <main module's build id>
//! f <entry> <code hash> <entry count>
//! b <block pc> <count>          (the blocks of the f above it)
//! t <block pc> <taken>          (its conditional branch, taken this often)
//! ```
//!
//! An instrumented build ([`Map`]) counts each block's entries and each conditional branch's taken edge; its counters
//! come back from play as raw counts ([`Map::read_raw`]).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// The first word of a profile's first line.
pub const MAGIC: &str = "longshot-guest-profile";
/// The format this crate writes and reads.
pub const FORMAT: u32 = 1;

/// Why a profile, a map or raw counts could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: String,
        /// Why.
        source: std::io::Error,
    },
    /// A line is not of the format.
    #[error("{what}: line {line}: {text:?}")]
    Format {
        /// Which file or text.
        what: String,
        /// Its line (from 1).
        line: usize,
        /// The line.
        text: String,
    },
}

/// A hash of a function's instruction words, in order (FNV-1a, 64 bits): its counts apply only while it matches.
pub fn code_hash(words: impl IntoIterator<Item = u32>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for w in words {
        for b in w.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// One function's counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Function {
    /// Its instruction words' [`code_hash`].
    pub hash: u64,
    /// How often it was entered (its entry block's count).
    pub entries: u64,
    /// Each block's count, by its first instruction's address.
    pub blocks: BTreeMap<u64, u64>,
    /// Each conditional branch's taken count, by its block's address.
    pub taken: BTreeMap<u64, u64>,
}

impl Function {
    /// All its blocks' counts together: how much it ran (a block a step), to rank the hot functions by.
    pub fn weight(&self) -> u64 {
        self.blocks.values().sum()
    }

    /// Each branch's taken count at most its block's count. The instrumented IR's counters are plain loads and
    /// stores, not atomic, so threads running the same block lose increments of either, and a run can record a branch
    /// taken more often than its block was entered; a profile file can say anything. Past its block's count, a taken
    /// count would make the not-taken edge's weight negative and the branch's weights overflow LLVM's 32 bits.
    pub fn clamp_taken(&mut self) {
        for (pc, t) in &mut self.taken {
            *t = (*t).min(self.blocks.get(pc).copied().unwrap_or(0));
        }
    }

    fn add(&mut self, o: &Function) {
        self.entries += o.entries;
        for (&pc, &n) in &o.blocks {
            *self.blocks.entry(pc).or_default() += n;
        }
        for (&pc, &n) in &o.taken {
            *self.taken.entry(pc).or_default() += n;
        }
    }
}

/// A profile: the counts of the functions that ran, of one build of the game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// The build of the game's main module it was recorded on (its build id): where another build's profile has no
    /// record of a function, that says nothing about whether it runs.
    pub build: String,
    /// The functions that ran, by entry.
    pub functions: BTreeMap<u64, Function>,
}

impl Profile {
    /// The function at `entry`, if its counts are for these words (its hash matches).
    pub fn function(&self, entry: u64, hash: u64) -> Option<&Function> {
        self.functions.get(&entry).filter(|f| f.hash == hash)
    }

    /// Whether the function at `entry` (of this hash) never ran in this profile's play: only said for the build the
    /// profile was recorded on (`build`), where a function with no record did not run. A record with no entry but a
    /// block that ran (code reached by a jump into its middle, as a tail call or a longjmp does) ran.
    pub fn never_ran(&self, build: &str, entry: u64, hash: u64) -> bool {
        match self.functions.get(&entry) {
            Some(f) => f.hash == hash && f.entries == 0 && f.weight() == 0,
            None => !self.build.is_empty() && self.build == build,
        }
    }

    /// `o`'s counts added to these: a function's when its hash is the same, else the newer (`o`'s) alone.
    pub fn merge(&mut self, o: &Profile) {
        if self.build.is_empty() {
            self.build = o.build.clone();
        }
        for (&e, f) in &o.functions {
            match self.functions.get_mut(&e) {
                Some(mine) if mine.hash == f.hash => mine.add(f),
                _ => {
                    self.functions.insert(e, f.clone());
                }
            }
        }
    }

    /// The text.
    pub fn to_text(&self) -> String {
        let mut s = format!("{MAGIC} {FORMAT} build {}\n", if self.build.is_empty() { "-" } else { &self.build });
        for (&e, f) in &self.functions {
            let _ = writeln!(s, "f {e:x} {:016x} {}", f.hash, f.entries);
            for (&pc, &n) in f.blocks.iter().filter(|&(_, &n)| n != 0) {
                let _ = writeln!(s, "b {pc:x} {n}");
            }
            for (&pc, &n) in f.taken.iter().filter(|&(_, &n)| n != 0) {
                let _ = writeln!(s, "t {pc:x} {n}");
            }
        }
        s
    }

    /// A profile from its text; `what` names it in errors.
    pub fn parse(text: &str, what: &str) -> Result<Profile, Error> {
        let bad = |line: usize, t: &str| Error::Format { what: what.to_string(), line, text: t.to_string() };
        let mut lines = text.lines().enumerate();
        let (_, head) = lines.next().ok_or_else(|| bad(1, ""))?;
        let h: Vec<&str> = head.split_whitespace().collect();
        if h.len() != 4 || h[0] != MAGIC || h[1] != FORMAT.to_string() || h[2] != "build" {
            return Err(bad(1, head));
        }
        let mut p = Profile { build: if h[3] == "-" { String::new() } else { h[3].to_string() }, ..Profile::default() };
        let mut cur: Option<u64> = None;
        for (i, line) in lines {
            let t: Vec<&str> = line.split_whitespace().collect();
            let hex = |s: &str| u64::from_str_radix(s, 16).map_err(|_| bad(i + 1, line));
            let dec = |s: &str| s.parse::<u64>().map_err(|_| bad(i + 1, line));
            match t.as_slice() {
                [] => {}
                ["f", e, h, n] => {
                    let e = hex(e)?;
                    p.functions.insert(e, Function { hash: hex(h)?, entries: dec(n)?, ..Function::default() });
                    cur = Some(e);
                }
                ["b", pc, n] | ["t", pc, n] => {
                    let f = cur.and_then(|e| p.functions.get_mut(&e)).ok_or_else(|| bad(i + 1, line))?;
                    let map = if t[0] == "b" { &mut f.blocks } else { &mut f.taken };
                    map.insert(hex(pc)?, dec(n)?);
                }
                _ => return Err(bad(i + 1, line)),
            }
        }
        p.functions.values_mut().for_each(Function::clamp_taken);
        Ok(p)
    }

    /// The profile in `path`.
    pub fn load(path: &Path) -> Result<Profile, Error> {
        let text = read(path)?;
        Profile::parse(&text, &path.display().to_string())
    }
}

/// The xz format's first bytes.
const XZ_MAGIC: &[u8] = b"\xFD7zXZ\x00";

/// A profile's or a map's text, from its file as it is or xz-compressed (the committed profile,
/// `assets/profiles/guest.profile.xz`: told by its first bytes, whatever its name).
fn read(path: &Path) -> Result<String, Error> {
    let io = |source| Error::Io { path: path.display().to_string(), source };
    let bytes = std::fs::read(path).map_err(io)?;
    let bytes = if bytes.starts_with(XZ_MAGIC) {
        let mut out = Vec::new();
        lzma_rs::xz_decompress(&mut &bytes[..], &mut out).map_err(|e| io(std::io::Error::other(e.to_string())))?;
        out
    } else {
        bytes
    };
    String::from_utf8(bytes).map_err(|e| io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))
}

/// What one counter of an instrumented build counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counter {
    /// The entries of a block (its first instruction's address).
    Block(u64),
    /// The taken edge of a block's conditional branch.
    Taken(u64),
}

/// One function's counters in its unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Counted {
    /// Its entry.
    pub entry: u64,
    /// Its [`code_hash`].
    pub hash: u64,
    /// Its counters: what each counts, and its index in the unit's array.
    pub counters: Vec<(Counter, u32)>,
}

/// An instrumented build's counters: for each unit (its name), the functions it counts. The lifter writes it beside
/// the units (`profile_map.txt`); the raw counts from play are read with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Map {
    /// The build of the game's main module.
    pub build: String,
    /// Each unit's name and functions.
    pub units: BTreeMap<String, Vec<Counted>>,
}

impl Map {
    /// The text: `build <id>`, then `u <unit>`, `f <entry> <hash>`, `b <pc> <index>` and `t <pc> <index>` lines.
    pub fn to_text(&self) -> String {
        let mut s = format!("build {}\n", if self.build.is_empty() { "-" } else { &self.build });
        for (unit, fs) in &self.units {
            let _ = writeln!(s, "u {unit}");
            for f in fs {
                let _ = writeln!(s, "f {:x} {:016x}", f.entry, f.hash);
                for &(c, i) in &f.counters {
                    let _ = match c {
                        Counter::Block(pc) => writeln!(s, "b {pc:x} {i}"),
                        Counter::Taken(pc) => writeln!(s, "t {pc:x} {i}"),
                    };
                }
            }
        }
        s
    }

    /// A map from its text.
    pub fn parse(text: &str, what: &str) -> Result<Map, Error> {
        let bad = |line: usize, t: &str| Error::Format { what: what.to_string(), line, text: t.to_string() };
        let mut m = Map::default();
        let mut unit: Option<String> = None;
        for (i, line) in text.lines().enumerate() {
            let t: Vec<&str> = line.split_whitespace().collect();
            let hex = |s: &str| u64::from_str_radix(s, 16).map_err(|_| bad(i + 1, line));
            let idx = |s: &str| s.parse::<u32>().map_err(|_| bad(i + 1, line));
            match t.as_slice() {
                [] => {}
                ["build", b] => m.build = if *b == "-" { String::new() } else { b.to_string() },
                ["u", name] => {
                    m.units.entry(name.to_string()).or_default();
                    unit = Some(name.to_string());
                }
                ["f", e, h] => {
                    let fs = unit.as_ref().and_then(|u| m.units.get_mut(u)).ok_or_else(|| bad(i + 1, line))?;
                    fs.push(Counted { entry: hex(e)?, hash: hex(h)?, counters: Vec::new() });
                }
                ["b", pc, n] | ["t", pc, n] => {
                    let f = unit
                        .as_ref()
                        .and_then(|u| m.units.get_mut(u))
                        .and_then(|fs| fs.last_mut())
                        .ok_or_else(|| bad(i + 1, line))?;
                    let c = if t[0] == "b" { Counter::Block(hex(pc)?) } else { Counter::Taken(hex(pc)?) };
                    f.counters.push((c, idx(n)?));
                }
                _ => return Err(bad(i + 1, line)),
            }
        }
        Ok(m)
    }

    /// The map in `path`.
    pub fn load(path: &Path) -> Result<Map, Error> {
        let text = read(path)?;
        Map::parse(&text, &path.display().to_string())
    }

    /// The profile the raw counts of a run give: host/guest_profile.c's text, `unit <name>` lines each followed by
    /// `<index> <count>` lines of the counters that ran.
    pub fn read_raw(&self, raw: &str, what: &str) -> Result<Profile, Error> {
        let bad = |line: usize, t: &str| Error::Format { what: what.to_string(), line, text: t.to_string() };
        let mut counts: BTreeMap<&str, BTreeMap<u32, u64>> = BTreeMap::new();
        let mut unit: Option<&str> = None;
        for (i, line) in raw.lines().enumerate() {
            let t: Vec<&str> = line.split_whitespace().collect();
            match t.as_slice() {
                [] => {}
                ["unit", name] => unit = Some(name),
                [idx, n] => {
                    let u = unit.ok_or_else(|| bad(i + 1, line))?;
                    let idx: u32 = idx.parse().map_err(|_| bad(i + 1, line))?;
                    let n: u64 = n.parse().map_err(|_| bad(i + 1, line))?;
                    *counts.entry(u).or_default().entry(idx).or_default() += n;
                }
                _ => return Err(bad(i + 1, line)),
            }
        }
        let mut p = Profile { build: self.build.clone(), ..Profile::default() };
        for (unit, fs) in &self.units {
            let Some(c) = counts.get(unit.as_str()) else { continue };
            for f in fs {
                let mut out = Function { hash: f.hash, ..Function::default() };
                for &(k, i) in &f.counters {
                    let n = c.get(&i).copied().unwrap_or(0);
                    match k {
                        Counter::Block(pc) => {
                            if pc == f.entry {
                                out.entries = n;
                            }
                            out.blocks.insert(pc, n);
                        }
                        Counter::Taken(pc) => {
                            out.taken.insert(pc, n);
                        }
                    }
                }
                out.clamp_taken();
                if out.entries > 0 || out.blocks.values().any(|&n| n > 0) {
                    p.functions.insert(f.entry, out);
                }
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests;

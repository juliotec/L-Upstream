//! The mods' ExeFS patches: IPS, IPS32 and IPSwitch (`.pchtxt`) files for the game's main, applied when the game is
//! built.
//!
//! Longshot compiles the game's code ahead of time, so a patch to its instructions cannot be applied when the game
//! runs: the patch files of the mods that are on are read at the build, and the image ([`apply`]) writes them into
//! main before it relocates it, as emulators and Atmosphere apply a patch file before the dynamic linker runs. The code
//! is then translated from the patched instructions, and the image holds the patched data. The game lists what was
//! applied in the Insert menu's Mods tab (game/mods.c reads `exefs_patches.txt`: [`write_list`]).
//!
//! The mods are manifest mods, the one mod format (a folder with `mod.json` in a profile's `mods/`): the Mod API's
//! layer finds them, with their order and switches, and hands [`collect`] their folders; a mod's patch files are its
//! `exefs/*.ips` and `*.pchtxt`. A mod in the emulators' layout (bare `romfs/`, `exefs/` and `cheats/` folders) is
//! never read here: `Longshot.exe mod import` turns it into a manifest mod once ([`crate::mod_layout`]). Where two files
//! write one byte the later wins. A file is for the game when its build id is main's: an IPS
//! file's name, a pchtxt's `@nsobid` (trailing zeros may be left out). IPS offsets count the NSO header (0x100 bytes)
//! before main's image; a pchtxt offset adds its `@flag offset_shift` first; a pchtxt line applies between `@enabled`
//! and `@disabled`, and `@stop` ends the file.
//!
//! A file is unsupported as a whole when a write falls outside main or into the dynamic linker's tables or a function
//! pointer it binds. Bytes into a word the dynamic linker fills with an explicit addend change nothing (the relocation
//! writes over them, as on the console). A write into the padding after the code (a code cave) makes that padding
//! code. A file that would stop one of the game's hook sites resolving is refused, naming the site (build-driver's
//! `clashes::refuse`).
//!
//! The formats' parse follows valentderah/totk-pc-recomp's tools/exefs_patches.py (MIT, Copyright (c) 2026 Valiantsin
//! Dzerakh), through Longshot's earlier Python builder.

use crate::nso::{Nso, RelocKind};
use build_core::{Error, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The bytes before main's image an IPS offset counts.
pub const NSO_HEADER: i64 = 0x100;

/// FNV-1a, 32 bits: a patch file's fingerprint in the list.
pub fn fnv32(data: &[u8]) -> u32 {
    data.iter().fold(0x811C_9DC5u32, |h, &b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

// ---- the mods

/// A mod whose patch files the build reads: its name (in the list the game reads, `exefs_patches.txt`), its folder,
/// and whether it is on. The Mod API's layer makes them, from a profile's manifest mods.
pub type Mod = (String, PathBuf, bool);

/// The `.ips` and `.pchtxt` files of a mod's exefs folder, by name.
pub fn patch_files(mod_path: &Path) -> Vec<String> {
    let d = mod_path.join("exefs");
    let Ok(listing) = std::fs::read_dir(&d) else { return Vec::new() };
    let mut names: Vec<String> = listing
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".ips") || l.ends_with(".pchtxt")
        })
        .collect();
    names.sort();
    names
}

// ---- the formats: offset in main's image -> byte

/// Writes: offset in main's image (may be negative or past it: refused later) to byte.
pub type Writes = BTreeMap<i64, u8>;

/// Whether `name` (hex, any case, trailing zeros left out or not) is main's build id.
pub fn same_build(name: &str, build_id: &str) -> bool {
    let n = name.trim().to_lowercase();
    !n.is_empty() && n.len() <= 64 && n.bytes().all(|c| c.is_ascii_hexdigit()) && {
        let pad = |s: &str| format!("{s:0<64}");
        pad(&n) == pad(&build_id.to_lowercase())
    }
}

/// The largest patch file read (an IPS or pchtxt of a mod is a few KB): a larger one is not a patch of main's.
pub const MAX_PATCH_FILE: u64 = 4 << 20;
/// The most bytes one file may write, runs and repeats counted (an IPS run of 64 KB is 3 bytes of the file).
pub const MAX_WRITTEN: usize = 1 << 20;

/// A file's writes, each inside main's image of `size` bytes: the first outside it, or past [`MAX_WRITTEN`], stops the
/// file (an offset or a run nothing in main could mean: a damaged or hostile file, not a patch).
struct Bounded {
    writes: Writes,
    size: u64,
    written: usize,
}

impl Bounded {
    fn new(size: u64) -> Bounded {
        Bounded { writes: Writes::new(), size, written: 0 }
    }

    fn put(&mut self, at: i64, bytes: &[u8]) -> std::result::Result<(), String> {
        let end = at.checked_add(bytes.len() as i64);
        if at < 0 || end.is_none_or(|e| e as u64 > self.size) {
            return Err(format!("a write at {at:#x} is outside main ({:#x} bytes)", self.size));
        }
        self.written += bytes.len();
        if self.written > MAX_WRITTEN {
            return Err(format!("it writes more than {} MB", MAX_WRITTEN >> 20));
        }
        for (i, &b) in bytes.iter().enumerate() {
            self.writes.insert(at + i as i64, b);
        }
        Ok(())
    }
}

/// IPS (`PATCH` ... `EOF`, 3-byte offsets) or IPS32 (`IPS32` ... `EEOF`, 4-byte offsets), writing main's image of
/// `size` bytes.
pub fn parse_ips(data: &[u8], size: u64) -> std::result::Result<Writes, String> {
    let (width, end): (usize, &[u8]) = match data.get(..5) {
        Some(b"PATCH") => (3, b"EOF"),
        Some(b"IPS32") => (4, b"EEOF"),
        _ => return Err("not an IPS or IPS32 file".into()),
    };
    let be = |b: &[u8]| b.iter().fold(0u64, |v, &x| v << 8 | u64::from(x));
    let mut writes = Bounded::new(size);
    let mut at = 5;
    loop {
        // (the end marker, and at most a 3-byte truncation record after it)
        if data.get(at..at + end.len()) == Some(end) && at + end.len() + 3 >= data.len() {
            return Ok(writes.writes);
        }
        let cut = || "the IPS file is cut short".to_string();
        let off = be(data.get(at..at + width).ok_or_else(cut)?) as i64;
        let size = be(data.get(at + width..at + width + 2).ok_or_else(cut)?) as usize;
        at += width + 2;
        let body: Vec<u8> = if size > 0 {
            let b = data.get(at..at + size).ok_or_else(cut)?.to_vec();
            at += size;
            b
        } else {
            // (a run: 2-byte length, 1 byte)
            let run = be(data.get(at..at + 2).ok_or_else(cut)?) as usize;
            let b = *data.get(at + 2).ok_or_else(cut)?;
            at += 3;
            vec![b; run]
        };
        writes.put(off - NSO_HEADER, &body)?;
    }
}

/// Python's `int(text, 0)`: decimal, or 0x/0o/0b with digits; underscores between digits.
fn int0(text: &str) -> Option<i64> {
    let (neg, t) = match text.strip_prefix('-') {
        Some(t) => (true, t),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let lower = t.to_lowercase();
    let (radix, digits) = match lower.get(..2) {
        Some("0x") => (16, &lower[2..]),
        Some("0o") => (8, &lower[2..]),
        Some("0b") => (2, &lower[2..]),
        _ => (10, lower.as_str()),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    if radix == 10 && digits.len() > 1 && digits.starts_with('0') && !digits.trim_start_matches(['0', '_']).is_empty() {
        return None;
    }
    let v = i64::from_str_radix(&digits.replace('_', ""), radix).ok()?;
    Some(if neg { -v } else { v })
}

/// A pchtxt value's bytes: hex digits in byte order, or a quoted string with C escapes.
pub fn pchtxt_value(token: &str) -> std::result::Result<Vec<u8>, String> {
    if let Some(rest) = token.strip_prefix('"') {
        let body = rest
            .strip_suffix('"')
            .filter(|_| token.len() >= 2)
            .ok_or_else(|| format!("a string without its closing quote: {token}"))?;
        let mut out = Vec::new();
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\\' {
                let mut b = [0u8; 4];
                // (Python's unicode_escape reads the UTF-8 bytes as Latin-1: a letter past ASCII is its UTF-8 bytes)
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                continue;
            }
            let Some(e) = chars.next() else { return Err("a string ending in a backslash".into()) };
            match e {
                'n' => out.push(b'\n'),
                't' => out.push(b'\t'),
                'r' => out.push(b'\r'),
                '0'..='7' => {
                    let mut v = e.to_digit(8).unwrap_or(0);
                    for _ in 0..2 {
                        match chars.peek().and_then(|c| c.to_digit(8)) {
                            Some(d) => {
                                v = v * 8 + d;
                                chars.next();
                            }
                            None => break,
                        }
                    }
                    out.push(u8::try_from(v).map_err(|_| "an octal escape past 255".to_string())?);
                }
                'x' => {
                    let h: String = chars.by_ref().take(2).collect();
                    out.push(u8::from_str_radix(&h, 16).map_err(|_| format!("a bad \\x escape in {token}"))?);
                }
                'a' => out.push(7),
                'b' => out.push(8),
                'f' => out.push(12),
                'v' => out.push(11),
                '\\' | '\'' | '"' => out.push(e as u8),
                '\n' => {}
                other => {
                    out.push(b'\\');
                    let mut b = [0u8; 4];
                    out.extend_from_slice(other.encode_utf8(&mut b).as_bytes());
                }
            }
        }
        return Ok(out);
    }
    if !token.len().is_multiple_of(2) || token.is_empty() || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("not a hex value: {token}"));
    }
    Ok((0..token.len()).step_by(2).map(|i| u8::from_str_radix(&token[i..i + 2], 16).unwrap_or(0)).collect())
}

/// An IPSwitch file's build id (if it names one) and writes, into main's image of `size` bytes.
pub fn parse_pchtxt(text: &str, size: u64) -> std::result::Result<(Option<String>, Writes), String> {
    let (mut bid, mut shift, mut enabled, mut writes) = (None, 0i64, false, Bounded::new(size));
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("@stop") {
            break;
        }
        if let Some(b) = line.strip_prefix("@nsobid-") {
            bid = Some(b.trim().to_string());
        } else if line.starts_with("@enabled") {
            enabled = true;
        } else if line.starts_with("@disabled") {
            enabled = false;
        } else if line.starts_with("@flag") {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() >= 3 && f[1] == "offset_shift" {
                shift = int0(f[2]).ok_or_else(|| format!("a bad offset_shift: {}", f[2]))?;
            }
        } else if line.starts_with('@') || !enabled {
            continue;
        } else {
            // ([0-9A-Fa-f]+)\s+(".*"|\S+), the whole line: else a title in brackets or a line of text
            let hex_end = line.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(line.len());
            let rest = &line[hex_end..];
            let value = rest.trim_start();
            if hex_end == 0 || rest.len() == value.len() || value.is_empty() {
                continue;
            }
            let is_value = (value.starts_with('"') && value.len() >= 2 && value.ends_with('"'))
                || !value.contains(char::is_whitespace);
            if !is_value {
                continue;
            }
            let off = i64::from_str_radix(&line[..hex_end], 16)
                .ok()
                .and_then(|o| o.checked_add(shift)?.checked_sub(NSO_HEADER))
                .ok_or_else(|| format!("an offset past any module: {line}"))?;
            writes.put(off, &pchtxt_value(value)?)?;
        }
    }
    Ok((bid, writes.writes))
}

// ---- what a write meets in main

/// A patch file's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// It writes main's code (or both code and data).
    Code,
    /// It writes main's data only.
    Data,
    /// Every byte it writes is main's already, or is relocated over.
    Empty,
    /// For another version of the game.
    Other,
    /// Not supported as a whole (the note says why).
    Unsupported,
    /// It would stop a hook site (the note names it).
    Refused,
    /// Its mod is off.
    Off,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Code => "code",
            Kind::Data => "data",
            Kind::Empty => "empty",
            Kind::Other => "other",
            Kind::Unsupported => "unsupported",
            Kind::Refused => "refused",
            Kind::Off => "off",
        }
    }

    /// Whether its writes are applied.
    pub fn applies(self) -> bool {
        matches!(self, Kind::Code | Kind::Data)
    }
}

/// main as the patches meet it: its image as loaded (before relocation), the dynamic linker's tables and the words it
/// binds.
pub struct Main<'a> {
    nso: &'a Nso,
    code_end: u64,
    tables: Vec<(u64, u64)>,
    bound: BTreeMap<u64, RelocKind>,
}

impl<'a> Main<'a> {
    /// main's tables, from its module.
    pub fn new(nso: &'a Nso) -> Main<'a> {
        let d = |t| nso.dyn_value(t);
        let mut tables = Vec::new();
        let rel =
            u32::from_le_bytes(nso.image[nso.mod0 as usize + 4..nso.mod0 as usize + 8].try_into().unwrap_or_default())
                as i32;
        let start = nso.mod0.wrapping_add_signed(i64::from(rel));
        let mut end = start;
        while nso.image.get(end as usize..end as usize + 8).is_some_and(|b| b != [0; 8]) {
            end += 16;
        }
        tables.push((start, end + 16));
        use crate::nso::{
            DT_JMPREL, DT_PLTRELSZ, DT_RELA, DT_RELASZ, DT_RELR, DT_RELRSZ, DT_STRSZ, DT_STRTAB, DT_SYMTAB,
        };
        for (s, n) in [(DT_RELA, DT_RELASZ), (DT_JMPREL, DT_PLTRELSZ), (DT_RELR, DT_RELRSZ)] {
            if d(s) != 0 && d(n) != 0 {
                tables.push((d(s), d(s) + d(n)));
            }
        }
        if d(DT_SYMTAB) != 0 && d(DT_STRTAB) > d(DT_SYMTAB) {
            tables.push((d(DT_SYMTAB), d(DT_STRTAB)));
        }
        if d(DT_STRTAB) != 0 && d(DT_STRSZ) != 0 {
            tables.push((d(DT_STRTAB), d(DT_STRTAB) + d(DT_STRSZ)));
        }
        Main {
            nso,
            code_end: nso.segments[1].offset,
            tables,
            bound: nso.relocs.iter().map(|r| (r.offset, r.kind)).collect(),
        }
    }

    /// One file's writes: (kind, note, the writes kept: bytes that differ from main's).
    pub fn classify(&self, writes: &Writes) -> (Kind, String, Writes) {
        let size = self.nso.image.len() as i64;
        let mut keep = Writes::new();
        let mut relocated = 0;
        for (&off, &b) in writes {
            if !(0..size).contains(&off) {
                return (
                    Kind::Unsupported,
                    format!("it writes outside the game's code module at main+{off:X}"),
                    Writes::new(),
                );
            }
            let o = off as u64;
            if self.tables.iter().any(|&(a, e)| a <= o && o < e) {
                return (
                    Kind::Unsupported,
                    format!("it writes the dynamic linker's tables at main+{o:X}"),
                    Writes::new(),
                );
            }
            let word = (o.saturating_sub(7)..=o).find(|w| self.bound.contains_key(w));
            if let Some(w) = word {
                if self.bound[&w] == RelocKind::JumpSlot {
                    return (
                        Kind::Unsupported,
                        format!("it writes a function pointer the dynamic linker binds at main+{w:X}"),
                        Writes::new(),
                    );
                }
                // (a word the linker relocates: left out. With an explicit addend the linker writes over it, as on the
                // console; a DT_RELR word's addend was read from the module before the patch, and the image relocates
                // from that)
                relocated += usize::from(self.nso.image[o as usize] != b);
                continue;
            }
            if self.nso.image[o as usize] != b {
                keep.insert(off, b);
            }
        }
        let note = if relocated > 0 {
            format!(
                "{relocated} byte{} in words the dynamic linker relocates left out",
                if relocated == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        };
        if keep.is_empty() {
            return (Kind::Empty, note, keep);
        }
        let kind = if keep.keys().any(|&o| (o as u64) < self.code_end) { Kind::Code } else { Kind::Data };
        (kind, note, keep)
    }

    /// One patch file (its name, its bytes): (kind, note, writes).
    pub fn classify_file(&self, name: &str, raw: &[u8]) -> (Kind, String, Writes) {
        let parsed = if name.to_lowercase().ends_with(".ips") {
            let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if !same_build(&stem, &self.nso.build_id) {
                return (Kind::Other, String::new(), Writes::new());
            }
            parse_ips(raw, self.nso.image.len() as u64)
        } else {
            let text = String::from_utf8_lossy(raw);
            match parse_pchtxt(text.strip_prefix('\u{feff}').unwrap_or(&text), self.nso.image.len() as u64) {
                Ok((Some(bid), w)) if same_build(&bid, &self.nso.build_id) => Ok(w),
                Ok(_) => return (Kind::Other, String::new(), Writes::new()),
                Err(e) => Err(e),
            }
        };
        match parsed {
            Ok(w) => self.classify(&w),
            Err(e) => (Kind::Unsupported, e, Writes::new()),
        }
    }
}

/// One patch file, as the lift and the list see it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchFile {
    /// Its mod's name.
    #[serde(rename = "mod")]
    pub mod_name: String,
    /// The file's name in the mod's exefs folder.
    pub file: String,
    /// FNV-1a of its bytes, 8 hex digits.
    pub hash: String,
    /// What it is.
    pub kind: Kind,
    /// Why, for a file not applied as it is.
    pub note: String,
    /// Whether its mod is on.
    pub on: bool,
    /// Its writes, (offset in main's image, byte), by offset.
    pub writes: Vec<(u64, u8)>,
}

/// The patch files of `mods`, each classified, in their order (the clash check with the hook sites is the read stage's,
/// build-driver's clashes.rs). Every line said goes to `log`.
pub fn collect(main: &Nso, mods: &[Mod], log: &mut dyn FnMut(String)) -> Result<Vec<PatchFile>> {
    let files: Vec<(String, PathBuf, bool, String)> = mods
        .iter()
        .flat_map(|(n, p, on)| patch_files(p).into_iter().map(move |f| (n.clone(), p.clone(), *on, f)))
        .collect();
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let m = Main::new(main);
    let mut out = Vec::new();
    for (name, path, on, f) in files {
        let p = path.join("exefs").join(&f);
        let len = std::fs::metadata(&p).at(&p)?.len();
        let raw = if len > MAX_PATCH_FILE { Vec::new() } else { std::fs::read(&p).at(&p)? };
        let (kind, note, writes) = if !on {
            (Kind::Off, String::new(), Writes::new())
        } else if len > MAX_PATCH_FILE {
            (Kind::Unsupported, format!("{} MB: larger than any patch file", len >> 20), Writes::new())
        } else {
            m.classify_file(&f, &raw)
        };
        let shown = if !note.is_empty() {
            format!(" ({note})")
        } else if !writes.is_empty() {
            format!(" ({} bytes)", writes.len())
        } else {
            String::new()
        };
        log(format!("exefs: {:<11} {name}/exefs/{f}{shown}", kind.word()));
        out.push(PatchFile {
            mod_name: name,
            file: f,
            hash: format!("{:08x}", fnv32(&raw)),
            kind,
            note,
            on,
            writes: writes.into_iter().map(|(o, b)| (o as u64, b)).collect(),
        });
    }
    Ok(out)
}

/// `exefs_patches.txt` in the game folder: what the build applied, a line a file (game/mods.c exefs_status).
pub fn list_text(files: &[PatchFile]) -> String {
    let mut lines = vec![
        "# The mods' ExeFS patch files this game was built with (longshot-builder): mod, file, FNV-1a of its bytes, kind, note"
            .to_string(),
    ];
    lines.extend(
        files.iter().filter(|f| f.on).map(|f| {
            format!("{}\t{}\t{}\t{}\t{}", f.mod_name, f.file, f.hash, f.kind.word(), f.note.replace('\t', " "))
        }),
    );
    lines.join("\n") + "\n"
}

/// The list written (only when it changed).
pub fn write_list(files: &[PatchFile], path: &Path) -> Result<()> {
    build_core::fsx::write_if_changed(path, list_text(files).as_bytes()).map(|_| ())
}

/// The applied files as the JSON the lift's key and the image read (the code and data files only: the rest change
/// nothing in the game).
pub fn to_json(files: &[PatchFile]) -> Vec<u8> {
    let applied: Vec<&PatchFile> = files.iter().filter(|f| f.kind.applies()).collect();
    serde_json::to_vec(&applied).unwrap_or_default()
}

/// The files of [`to_json`]'s JSON.
pub fn from_json(bytes: &[u8]) -> Result<Vec<PatchFile>> {
    serde_json::from_slice(bytes).map_err(|e| Error::format("exefs patches", e.to_string()))
}

/// What [`apply`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Bytes written.
    pub bytes: usize,
    /// Files applied.
    pub files: usize,
    /// Where main's code ends now, when a code cave made the padding after it code.
    pub cave_end: Option<u64>,
}

/// The code and data files' writes into main's image, before it is relocated (the later file wins); the padding
/// after main's code that a cave was written into becomes code (its `.text` reaches the last word written).
pub fn apply(main: &mut Nso, files: &[PatchFile]) -> Result<Applied> {
    let (text_off, text_size) = (main.segments[0].offset, main.segments[0].size);
    let rodata = main.segments[1].offset;
    let mut done = Applied::default();
    let mut cave_end = 0u64;
    for f in files.iter().filter(|f| f.kind.applies()) {
        for &(off, b) in &f.writes {
            // (parse_ips and parse_pchtxt keep every write inside main's image: one past it is the builder's bug)
            let slot = main.image.get_mut(off as usize).ok_or_else(|| {
                Error::format(format!("{}'s {}", f.mod_name, f.file), format!("a write at {off:#x}, past main's image"))
            })?;
            *slot = b;
            done.bytes += 1;
            if (text_off + text_size..rodata).contains(&off) {
                cave_end = cave_end.max((off + 4) & !3);
            }
        }
        done.files += 1;
    }
    if cave_end != 0 {
        main.segments[0].size = cave_end - text_off;
        done.cave_end = Some(cave_end);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(records: &[(u32, &[u8])]) -> Vec<u8> {
        let mut out = b"PATCH".to_vec();
        for (off, body) in records {
            out.extend_from_slice(&off.to_be_bytes()[1..]);
            out.extend_from_slice(&(body.len() as u16).to_be_bytes());
            out.extend_from_slice(body);
        }
        out.extend_from_slice(b"EOF");
        out
    }

    #[test]
    fn ips_and_ips32() {
        let w = parse_ips(&ips(&[(0x140, &[1, 2])]), 0x1000).unwrap();
        assert_eq!(w.into_iter().collect::<Vec<_>>(), [(0x40, 1), (0x41, 2)]);
        let mut run = b"IPS32".to_vec();
        run.extend_from_slice(&[0, 0, 1, 0, 0, 0, 0, 3, 9]);
        run.extend_from_slice(b"EEOF");
        assert_eq!(parse_ips(&run, 0x1000).unwrap().len(), 3);
        assert!(parse_ips(b"PATCH\x00\x01", 0x1000).is_err());
        assert!(parse_ips(b"NOPE!", 0x1000).is_err());
    }

    #[test]
    fn a_write_outside_main_or_a_bomb_of_runs_stops_the_file() {
        assert!(parse_ips(&ips(&[(0x100 + 0xfff, &[1, 2])]), 0x1000).is_err(), "one byte past main's end");
        assert!(parse_ips(&ips(&[(0x10, &[1])]), 0x1000).is_err(), "inside the NSO header, before main's image");
        // (runs of 64 KB, each 8 bytes of the file: 20 of them write 1.3 MB)
        let mut bomb = b"PATCH".to_vec();
        for i in 0..20u32 {
            bomb.extend_from_slice(&(0x100 + (i % 16) * 0x10000).to_be_bytes()[1..]);
            bomb.extend_from_slice(&[0, 0, 0xff, 0xff, 7]);
        }
        bomb.extend_from_slice(b"EOF");
        let e = parse_ips(&bomb, 1 << 22).unwrap_err();
        assert!(e.contains("more than"), "{e}");
        assert!(parse_pchtxt("@enabled\n7fffffffffffffff 00\n", 0x1000).is_err(), "an offset that overflows");
        assert!(parse_pchtxt("@flag offset_shift 0x7fffffffffffffff\n@enabled\n10 00\n", 0x1000).is_err());
        assert!(parse_pchtxt("@enabled\n10ff 0000\n", 0x1000).is_err(), "past main");
        assert!(parse_pchtxt("@enabled\n10fe 0000\n", 0x1000).is_ok(), "main's last two bytes");
    }

    #[test]
    fn pchtxt() {
        let text = "@nsobid-E0\n@flag offset_shift 0x100\n// a comment\n[a title]\n@enabled\n00000040 1F2003D5 // nop\n44 \"A\\n\"\n@disabled\n48 FF\n@enabled\n@stop\n4c FF\n";
        let (bid, w) = parse_pchtxt(text, 0x1000).unwrap();
        assert_eq!(bid.as_deref(), Some("E0"));
        assert_eq!(
            w.into_iter().collect::<Vec<_>>(),
            [(0x40, 0x1f), (0x41, 0x20), (0x42, 0x03), (0x43, 0xd5), (0x44, b'A'), (0x45, b'\n')]
        );
        assert!(pchtxt_value("ABC").is_err() && pchtxt_value("\"open").is_err());
        assert_eq!(int0("0x10"), Some(16));
        assert_eq!(int0("-8"), Some(-8));
        assert_eq!(int0("1_000"), Some(1000));
        assert_eq!(int0("08"), None);
    }

    #[test]
    fn builds_match_with_zeros_left_off() {
        assert!(same_build("E0F3", "e0f3000000"));
        assert!(!same_build("e0f4", "e0f3"));
        assert!(!same_build("", "e0f3") && !same_build("xyz", "e0f3"));
    }

    #[test]
    fn the_mods_given_are_read_in_their_order_an_off_one_left_out() {
        let d = crate::nup::tests::scratch("patches-mods");
        let n = Nso::parse("main".into(), &crate::nso::tests::synthetic(0xe0, false)).unwrap();
        for m in ["b", "a", "c"] {
            std::fs::create_dir_all(d.join(m).join("exefs")).unwrap();
            std::fs::write(d.join(m).join("exefs/E0.ips"), ips(&[(0x140, &[0])])).unwrap();
        }
        std::fs::write(d.join("a/exefs/notes.txt"), b"not a patch file").unwrap();
        let mods: Vec<Mod> =
            [("b", true), ("a", false), ("c", true)].map(|(m, on)| (format!("owner/{m}"), d.join(m), on)).to_vec();
        let mut said = Vec::new();
        let files = collect(&n, &mods, &mut |l| said.push(l)).unwrap();
        let got: Vec<(&str, &str, Kind)> =
            files.iter().map(|f| (f.mod_name.as_str(), f.file.as_str(), f.kind)).collect();
        assert_eq!(
            got,
            [("owner/b", "E0.ips", Kind::Code), ("owner/a", "E0.ips", Kind::Off), ("owner/c", "E0.ips", Kind::Code)]
        );
        assert_eq!(said.len(), 3, "{said:?}");
        assert!(collect(&n, &[], &mut |_| unreachable!()).unwrap().is_empty());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn writes_are_classified_against_main() {
        let n = Nso::parse("main".into(), &crate::nso::tests::synthetic(0xe0, false)).unwrap();
        let m = Main::new(&n);
        let w = |v: &[(i64, u8)]| v.iter().copied().collect::<Writes>();
        assert_eq!(m.classify(&w(&[(0x40, 0)])).0, Kind::Code);
        assert_eq!(m.classify(&w(&[(0x330, 7)])).0, Kind::Data);
        assert_eq!(m.classify(&w(&[(0x40, 0x1f)])).0, Kind::Empty, "the byte is main's already");
        assert_eq!(m.classify(&w(&[(-1, 0)])).0, Kind::Unsupported);
        assert_eq!(m.classify(&w(&[(0x200, 1)])).0, Kind::Unsupported, "the dynamic section");
        assert_eq!(m.classify(&w(&[(0x309, 1)])).0, Kind::Unsupported, "a bound function pointer");
        let (k, note, _) = m.classify(&w(&[(0x301, 9)]));
        assert_eq!((k, note.as_str()), (Kind::Empty, "1 byte in words the dynamic linker relocates left out"));
        assert_eq!(m.classify_file("E0.ips", &ips(&[(0x140, &[0])])).0, Kind::Code);
        assert_eq!(m.classify_file("E1.ips", &ips(&[(0x140, &[0])])).0, Kind::Other);
    }

    #[test]
    fn a_cave_after_the_code_becomes_code() {
        let mut n = Nso::parse("main".into(), &crate::nso::tests::synthetic(0xe0, false)).unwrap();
        // (.text is 0..0x100 and .rodata starts at 0x100 here: shrink .text to make padding)
        n.segments[0].size = 0xf0;
        let files = vec![PatchFile {
            mod_name: "m".into(),
            file: "E0.ips".into(),
            hash: "0".into(),
            kind: Kind::Code,
            note: String::new(),
            on: true,
            writes: vec![(0xf4, 0xaa)],
        }];
        let done = apply(&mut n, &files).unwrap();
        assert_eq!(done, Applied { bytes: 1, files: 1, cave_end: Some(0xf8) });
        assert_eq!((n.image[0xf4], n.segments[0].size), (0xaa, 0xf8));
        assert_eq!(from_json(&to_json(&files)).unwrap(), files);
        assert!(list_text(&files).ends_with("m\tE0.ips\t0\tcode\t\n"));
    }
}

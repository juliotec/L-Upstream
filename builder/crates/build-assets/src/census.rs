//! Every shader program in the game's archives, for the precompile before the first start: the game folder's
//! `longshot_shaders.bin`, which `gpu/vk/vk_shader.cpp` translates (it is made from the player's own game, and stays
//! on their PC).
//!
//! The walk: `Shader/` (`.bfsha` programs, and `ApplicationPackage`'s agl archives), `Layout2Shader/`, `Lib/` (agl, gsys
//! and terrain archives, agl's networks), `Movie/` (the movie upscaler's networks) and `Effect/` (particle sets, each a
//! BNSH inside a byml), SARCs opened and zstd unpacked on the way. A BNSH's variations are programs: which stages go
//! together. agl's shader archives (BAHS), its networks (NDSR) and the upscaler's (NDVE) are read each in its own way
//! (below). A stage is its control section (0xd8 bytes) and the code area it sizes, the bytes the game hands the GPU:
//! what the back end keys it by, so the same stage in two archives is one.
//!
//! After `tools/shader_census.py` of TotK Recomp (MIT), itself after MacNativeGames' `prepare_shader_seed.py` (MIT):
//! the BNSH layout and the walk of its variations.
//!
//! ```text
//! "HKSEED01", u32 programs, u32 stages; then the stages, each once:
//!     u64 key, u32 stage, u32 control bytes, u32 code bytes, control, code
//! then the programs: u32 count, u32 stage indices
//! ```
//!
//! A file or container this cannot read is said and skipped; the programs found before it in the same file stay.

use crate::romfs::Romfs;
use crate::zs::{Decoder, Dictionaries};
use build_core::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The shader list, in the game folder.
pub const FILE: &str = "longshot_shaders.bin";
/// The romfs folders the census walks.
pub const FOLDERS: [&str; 5] = ["Shader", "Layout2Shader", "Lib", "Movie", "Effect"];
const CONTROL_MAGIC: [u8; 4] = [0x34, 0x12, 0x76, 0x98];
/// The stages a variation's program points at (a 0 offset: none), as far as the structure reaches.
const SLOTS: [usize; 9] = [8, 16, 24, 32, 40, 48, 56, 64, 72];

/// One stage: its kind (the control's +0x56), its control section and its code.
type Stage = (u16, Vec<u8>, Vec<u8>);

/// The most bytes one file of the romfs may unpack to in all, its archives' files together (a SARC holding itself, or
/// frames that unpack to many times their size, would otherwise grow without end).
const FILE_BUDGET: usize = 1 << 30;
/// How deep SARCs may nest in one file (the game's: one inside another at most).
const SARC_DEPTH: usize = 3;

/// A file's programs as they are found: each stage once, by where it is (a structure that names one stage many times
/// is not one copy each), and the programs as their stages' places in that list.
#[derive(Debug, Default)]
struct Found {
    stages: Vec<Stage>,
    at: HashMap<(usize, usize, usize, usize, usize), u32>,
    programs: Vec<Vec<u32>>,
    /// (the container now: a stage's place is in it)
    container: usize,
}

impl Found {
    /// The stage at (control, control bytes, code, code bytes) of the container, made by `make` the first time.
    fn stage(&mut self, place: (usize, usize, usize, usize), make: impl FnOnce() -> Stage) -> u32 {
        let key = (self.container, place.0, place.1, place.2, place.3);
        if let Some(&id) = self.at.get(&key) {
            return id;
        }
        self.stages.push(make());
        let id = (self.stages.len() - 1) as u32;
        self.at.insert(key, id);
        id
    }
}

/// A read past the data's end: the container is not one this reads.
#[derive(Debug)]
struct Short;

type Got<T> = std::result::Result<T, Short>;

fn u16_at(d: &[u8], at: usize) -> Got<u16> {
    Ok(u16::from_le_bytes(d.get(at..at.checked_add(2).ok_or(Short)?).ok_or(Short)?.try_into().map_err(|_| Short)?))
}

fn u32_at(d: &[u8], at: usize) -> Got<u32> {
    Ok(u32::from_le_bytes(d.get(at..at.checked_add(4).ok_or(Short)?).ok_or(Short)?.try_into().map_err(|_| Short)?))
}

fn i32_at(d: &[u8], at: usize) -> Got<i32> {
    Ok(u32_at(d, at)? as i32)
}

fn u64_at(d: &[u8], at: usize) -> Got<u64> {
    Ok(u64::from_le_bytes(d.get(at..at.checked_add(8).ok_or(Short)?).ok_or(Short)?.try_into().map_err(|_| Short)?))
}

/// `d[a..b]`, clamped to `d` as Python's slices are.
fn slice(d: &[u8], a: usize, b: usize) -> &[u8] {
    let a = a.min(d.len());
    &d[a..b.clamp(a, d.len())]
}

/// Each place `needle` is in `hay`, not overlapping (as `re.finditer`).
fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + needle.len() <= hay.len() {
        match hay[at..].windows(needle.len()).position(|w| w == needle) {
            Some(i) => {
                out.push(at + i);
                at += i + needle.len().max(1);
            }
            None => break,
        }
    }
    out
}

/// Each variation of a BNSH (its offsets are its own) as its stages.
fn bnsh_programs(data: &[u8], out: &mut Found) -> Got<()> {
    if data.get(..4) != Some(b"BNSH") || data.get(12..14) != Some(&[0xFF, 0xFE]) || data.get(96..100) != Some(b"grsc") {
        return Err(Short);
    }
    let size = u32_at(data, 28)? as usize;
    let data = slice(data, 0, size);
    let (count, variations) = (u32_at(data, 124)? as usize, u64_at(data, 128)? as usize);
    // (a variation's record is 64 bytes: no more of them than the data holds)
    for i in 0..count.min(data.len() / 64) {
        let program = u64_at(data, variations.checked_add(i * 64 + 16).ok_or(Short)?)? as usize;
        if program == 0 || program.checked_add(80).is_none_or(|e| e > size) {
            continue;
        }
        let mut stages = Vec::new();
        for rel in SLOTS {
            let offset = u64_at(data, program + rel)? as usize;
            if offset == 0 || offset.checked_add(32).is_none_or(|e| e > size) {
                continue;
            }
            let (control_off, code_off) = (u64_at(data, offset + 8)? as usize, u64_at(data, offset + 16)? as usize);
            let (code_size, control_size) = (u32_at(data, offset + 24)? as usize, u32_at(data, offset + 28)? as usize);
            if !(0x60..=0x1000).contains(&control_size)
                || control_off.checked_add(control_size).is_none_or(|e| e > size)
                || code_off.checked_add(code_size).is_none_or(|e| e > size)
            {
                continue;
            }
            let control = slice(data, control_off, control_off + control_size);
            if control.get(..4) != Some(&CONTROL_MAGIC) {
                continue;
            }
            let stage = u16_at(control, 0x56)?;
            let code = slice(data, code_off, code_off + code_size);
            stages.push(
                out.stage((control_off, control_size, code_off, code_size), || {
                    (stage, control.to_vec(), code.to_vec())
                }),
            );
        }
        if !stages.is_empty() {
            out.programs.push(stages);
        }
    }
    Ok(())
}

/// The 0xd8-byte NVN2 control section at `c`, and its code area's size (+0x44).
fn control_at(data: &[u8], c: usize) -> Option<(&[u8], usize)> {
    let control = data.get(c..c.checked_add(0xd8)?)?;
    if control[..4] != CONTROL_MAGIC || u16::from_le_bytes([control[0x56], control[0x57]]) > 7 {
        return None;
    }
    Some((control, u32::from_le_bytes(control[0x44..0x48].try_into().ok()?) as usize))
}

fn stage(control: &[u8], code: &[u8]) -> Stage {
    (u16::from_le_bytes([control[0x56], control[0x57]]), control.to_vec(), code.to_vec())
}

/// agl's shader archive (`.sharcb`, magic BAHS). Header +0x14 u32 the data start, +0x18 the code blob's size, +0x1c its
/// start. At data +0x150: u32 the program table (from there), u32 the stage count; the stages follow from data +0x158,
/// each {u32 bytes to the next, u32 kind, u32, u32, u64 its code area's offset in the blob, u32 control bytes, s32 its
/// control section from there}. The program table: u32 bytes, u32 programs, each {u32 bytes, u32 name bytes, u32 its
/// kinds (a bit each), u32 its first stage, name, options}; a program's variations are its run of stages up to the
/// next program's first, one stage of each of its kinds per variation.
fn bahs_programs(data: &[u8], out: &mut Found) -> Got<()> {
    let add = |a: usize, b: usize| a.checked_add(b).ok_or(Short);
    let (base, blob) = (u32_at(data, 0x14)? as usize, u32_at(data, 0x1c)? as usize);
    let (table, count) = (u32_at(data, add(base, 0x150)?)? as usize, u32_at(data, add(base, 0x154)?)? as usize);
    let mut at = add(base, 0x158)?;
    // (a stage's record is 32 bytes at least: no more of them than the data holds)
    let count = count.min(data.len() / 32);
    let mut stages = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let step = u32_at(data, at)? as usize;
        if step < 32 {
            return Err(Short);
        }
        let offset = u64_at(data, add(at, 16)?)? as usize;
        let rel = i32_at(data, add(at, 28)?)? as isize;
        let c = (at as isize).checked_add(16 + rel).ok_or(Short)?.max(0) as usize;
        let (control, end) = control_at(data, c).ok_or(Short)?;
        let code = add(blob, offset)?;
        if add(code, end)? > data.len() {
            return Err(Short);
        }
        stages.push(out.stage((c, 0xd8, code, end), || stage(control, &data[code..code + end])));
        at = add(at, step)?;
    }
    let table = add(add(table, base)?, 0x150)?;
    let mut at = add(table, 8)?;
    let mut heads = Vec::new();
    // (a program's head is 16 bytes at least: one shorter is not one, and no more of them than the data holds)
    for _ in 0..(u32_at(data, add(table, 4)?)? as usize).min(data.len() / 16) {
        let (size, kinds, first) =
            (u32_at(data, at)? as usize, u32_at(data, add(at, 8)?)?, u32_at(data, add(at, 12)?)? as usize);
        if size < 16 {
            return Err(Short);
        }
        heads.push((first, kinds.count_ones() as usize));
        at = add(at, size)?;
    }
    heads.sort();
    for (i, &(first, width)) in heads.iter().enumerate() {
        let last = heads.get(i + 1).map_or(count, |h| h.0);
        let mut v = first;
        while v < last {
            out.programs.push(stages[v.min(stages.len())..v.saturating_add(width).min(stages.len())].to_vec());
            if v >= stages.len() {
                break; // (past the stages every one is the same empty program: one is all of them)
            }
            v = v.saturating_add(width.max(1));
        }
    }
    Ok(())
}

/// agl's networks (`agl_ndsr*.pck`, magic NDSR; also inside NDVE at +0x28): u32 +0x0c the item count, from +0x14 each
/// item {u32 offset from the NDSR, u32 bytes << 4 | type}. Types 0 / 3 / 6 are a vertex / fragment / compute control
/// section, the item after each (1 / 4 / 7) its code; a vertex and the fragment after it are a program, a compute
/// stage is one alone.
fn ndsr_programs(data: &[u8], base: usize, out: &mut Found) -> Got<()> {
    let n = (u32_at(data, base + 0x0c)? as usize).min(data.len() / 8);
    let mut items = Vec::with_capacity(n.min(1 << 16));
    for i in 0..n {
        items.push((u32_at(data, base + 0x14 + 8 * i)? as usize, u32_at(data, base + 0x18 + 8 * i)?));
    }
    let mut graphics: Vec<u32> = Vec::new();
    for w in items.windows(2) {
        let ((c, ct), (x, xt)) = (w[0], w[1]);
        if ![0, 3, 6].contains(&(ct & 15)) || xt & 15 != (ct & 15) + 1 {
            continue;
        }
        let Some((control, end)) = control_at(data, base + c) else { continue };
        if (base + x).checked_add(end).is_none_or(|e| e > data.len()) {
            continue;
        }
        let s = out.stage((base + c, 0xd8, base + x, end), || stage(control, &data[base + x..base + x + end]));
        match ct & 15 {
            6 => out.programs.push(vec![s]),
            0 => {
                if !graphics.is_empty() {
                    out.programs.push(std::mem::take(&mut graphics));
                }
                graphics = vec![s];
            }
            _ if !graphics.is_empty() => graphics.push(s),
            _ => {}
        }
    }
    if !graphics.is_empty() {
        out.programs.push(graphics);
    }
    Ok(())
}

/// The movie upscaler's networks (`Movie/Ndve`, magic NDVE): an NDSR at +0x28, then tables {u32 stages, u32 blobs,
/// blobs x {u32 offset, u32 bytes}, stages x {u32 control section from the table, u32 code from the control, u32 code
/// bytes, u32 0}}. Each control's entry is found from its code bytes (the pair "bytes, 0") and checked against its
/// table's header.
fn ndve_programs(data: &[u8], out: &mut Found) -> Got<()> {
    if data.get(0x28..0x2c) == Some(b"NDSR") {
        ndsr_programs(data, 0x28, out)?;
    }
    let controls: Vec<(usize, &[u8], usize)> = find_all(data, &CONTROL_MAGIC)
        .into_iter()
        .filter_map(|c| control_at(data, c).map(|(control, end)| (c, control, end)))
        .collect();
    // (every place of each pair "bytes, 0" the controls look for, found in one pass: one search a control was
    // the data's size times the controls)
    let pairs = pairs_of(data, controls.iter().map(|&(_, _, end)| end as u32).collect());
    for (c, control, end) in controls {
        for &e in pairs.get(&(end as u32)).map_or(&[][..], |v| v.as_slice()) {
            let Some(entry) = e.checked_sub(8) else { continue };
            if entry % 4 != 0 {
                continue;
            }
            let (to_control, to_code) = (u32_at(data, entry)? as usize, u32_at(data, entry + 4)? as usize);
            if to_control == 0
                || to_control > c
                || (c - to_control) % 4 != 0
                || c.checked_add(to_code).and_then(|x| x.checked_add(end)).is_none_or(|x| x > data.len())
            {
                continue;
            }
            let table = c - to_control;
            let (n, blobs) = (u32_at(data, table)? as usize, u32_at(data, table + 4)? as usize);
            let first = table + 8 + 8 * blobs.min(16);
            if 0 < n && n < 256 && blobs < 16 && entry >= first && (entry - first) % 16 == 0 && (entry - first) / 16 < n
            {
                let s =
                    out.stage((c, 0xd8, c + to_code, end), || stage(control, &data[c + to_code..c + to_code + end]));
                out.programs.push(vec![s]);
                break;
            }
        }
    }
    Ok(())
}

/// Each place of the 8 bytes "w, 0" (u32s, little-endian) in `data`, for each w of `wanted`, not overlapping (as
/// [`find_all`] finds each).
fn pairs_of(data: &[u8], wanted: HashSet<u32>) -> HashMap<u32, Vec<usize>> {
    let mut out: HashMap<u32, Vec<usize>> = HashMap::new();
    if data.len() < 8 {
        return out;
    }
    for at in 0..=data.len() - 8 {
        if data[at + 4..at + 8] != [0; 4] {
            continue;
        }
        let w = u32::from_le_bytes(data[at..at + 4].try_into().expect("4 bytes"));
        if !wanted.contains(&w) {
            continue;
        }
        let places = out.entry(w).or_default();
        if places.last().is_none_or(|&last| at >= last + 8) {
            places.push(at);
        }
    }
    out
}

/// Each BNSH inside the data (`.bfsha`, and Effect's particle sets: a BNSH inside a byml).
fn bnsh_all(data: &[u8], out: &mut Found) -> Got<()> {
    for at in find_all(data, b"BNSH") {
        if data.get(at + 96..at + 100) == Some(b"grsc") {
            let size = u32_at(data, at + 28)? as usize;
            bnsh_programs(slice(data, at, at.saturating_add(size)), out)?;
            out.container += 1; // (each BNSH's places are its own)
        }
    }
    Ok(())
}

/// One container's programs, all of them or none (a container this cannot read is skipped whole): into `out`, its
/// place there given back when it is skipped.
fn programs_in(data: &[u8], out: &mut Found) -> Got<()> {
    let mut mine = Found { container: out.container, ..Found::default() };
    match data.get(..4) {
        Some(b"BAHS") => bahs_programs(data, &mut mine)?,
        Some(b"NDSR") => ndsr_programs(data, 0, &mut mine)?,
        Some(b"NDVE") => ndve_programs(data, &mut mine)?,
        _ => bnsh_all(data, &mut mine)?,
    }
    // (its stages after the file's so far, each once: a container's places are its own)
    let first = out.stages.len() as u32;
    out.stages.extend(mine.stages);
    out.programs.extend(mine.programs.into_iter().map(|p| p.into_iter().map(|i| i + first).collect()));
    out.container = mine.container + 1;
    Ok(())
}

/// The files of a SARC as the census reads them (the data start at +0x0c, the count at +0x1a, the table at +0x20).
/// A file must start past the table (one that starts inside it is the archive's own bytes: a SARC holding itself).
fn sarc_files(data: &[u8]) -> Got<Vec<&[u8]>> {
    let start = u32_at(data, 0x0c)? as usize;
    let count = usize::from(u16_at(data, 0x1a)?);
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let (st, end) = (u32_at(data, 0x28 + k * 16)? as usize, u32_at(data, 0x2c + k * 16)? as usize);
        if start + st < 0x20 + 16 * count {
            return Err(Short);
        }
        out.push(slice(data, start + st, start + end));
    }
    Ok(out)
}

/// The containers of a file: itself, or each file when it is an archive (unpacked on the way), each handed to `f`.
/// Stops at the first file that does not unpack, at SARCs nested deeper than [`SARC_DEPTH`], and once the file's
/// archives have unpacked to more than `budget` bytes.
fn blobs(
    data: Vec<u8>,
    decoder: &mut Decoder,
    depth: usize,
    budget: &mut usize,
    f: &mut dyn FnMut(&[u8]),
) -> std::result::Result<(), String> {
    if !data.starts_with(b"SARC") {
        f(&data);
        return Ok(());
    }
    if depth >= SARC_DEPTH {
        return Err("SARCs nested too deep".to_string());
    }
    let inner = sarc_files(&data).map_err(|_| "a SARC cut short".to_string())?;
    for file in inner {
        let file = decoder.unpack(file, "a file of the archive").map_err(|_| "no dictionary fits".to_string())?;
        *budget = budget.checked_sub(file.len()).ok_or_else(|| "unpacks to more than 1 GB".to_string())?;
        blobs(file, decoder, depth + 1, budget, f)?;
    }
    Ok(())
}

/// The files the census reads, in its order: each folder of [`FOLDERS`] walked by name, a folder's files before its
/// folders, its `.zs`, `.bfsha`, `.sarc` and `.pck` files.
pub fn archives(romfs: &dyn Romfs) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for folder in FOLDERS {
        let mut files: Vec<String> = romfs
            .files_under(folder)?
            .into_iter()
            .filter(|f| {
                let n = f.rsplit('/').next().unwrap_or_default();
                n.ends_with(".zs") || [".bfsha", ".sarc", ".pck"].iter().any(|s| n.contains(s))
            })
            .collect();
        files.sort_by(|a, b| walk_order(a, b));
        out.extend(files);
    }
    Ok(out)
}

/// The order a walk of the folders meets two files in: by name, but a folder's own files before what is in its
/// folders.
fn walk_order(a: &str, b: &str) -> std::cmp::Ordering {
    let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
    for i in 0..pa.len().min(pb.len()) {
        if pa[i] != pb[i] {
            let (fa, fb) = (i + 1 == pa.len(), i + 1 == pb.len());
            return fb.cmp(&fa).then(pa[i].cmp(pb[i]));
        }
    }
    pa.len().cmp(&pb.len())
}

/// What the census found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Census {
    /// The stages, each once: (key, stage, control, code).
    pub stages: Vec<(u64, u16, Vec<u8>, Vec<u8>)>,
    /// The programs, each once: their stages' indices, sorted.
    pub programs: Vec<Vec<u32>>,
    /// The files and containers skipped, each with why.
    pub skipped: Vec<String>,
}

impl Census {
    /// The list's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = b"HKSEED01".to_vec();
        out.extend_from_slice(&(self.programs.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.stages.len() as u32).to_le_bytes());
        for (key, stage, control, code) in &self.stages {
            out.extend_from_slice(&key.to_le_bytes());
            out.extend_from_slice(&u32::from(*stage).to_le_bytes());
            out.extend_from_slice(&(control.len() as u32).to_le_bytes());
            out.extend_from_slice(&(code.len() as u32).to_le_bytes());
            out.extend_from_slice(control);
            out.extend_from_slice(code);
        }
        for ids in &self.programs {
            out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
            for i in ids {
                out.extend_from_slice(&i.to_le_bytes());
            }
        }
        out
    }
}

/// One file's programs and what of it was skipped.
type FileFound = (Found, Vec<String>);

/// One file's programs (and what was skipped), in its containers' order.
fn file_programs(romfs: &dyn Romfs, rel: &str, decoder: &mut Decoder) -> FileFound {
    let name = rel.rsplit('/').next().unwrap_or_default().to_string();
    let (mut found, mut skipped) = (Found::default(), Vec::new());
    let data = match romfs.read(rel) {
        Ok(d) => d,
        Err(e) => return (found, vec![format!("{name} skipped: {e}")]),
    };
    let data = match decoder.unpack(&data, &name) {
        Ok(d) => d,
        Err(_) => return (found, vec![format!("{name} skipped: no dictionary fits")]),
    };
    let mut k = 0;
    let mut budget = FILE_BUDGET.saturating_sub(data.len());
    let walked = blobs(data, decoder, 0, &mut budget, &mut |blob| {
        match programs_in(blob, &mut found) {
            Ok(()) => {}
            Err(Short) => {
                skipped.push(format!("{name} container {k} {:?} skipped", String::from_utf8_lossy(slice(blob, 0, 4))))
            }
        }
        k += 1;
    });
    if let Err(e) = walked {
        skipped.push(format!("{name} skipped: {e}"));
    }
    (found, skipped)
}

/// The census of `romfs`, its files read by `jobs` workers (the list is the same for any number).
pub fn census(romfs: &dyn Romfs, jobs: usize, progress: &(dyn Fn(f64) + Sync)) -> Result<Census> {
    let dicts = Dictionaries::load(romfs)?;
    let paths = archives(romfs)?;
    let results: Vec<Mutex<Option<FileFound>>> = paths.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, paths.len().max(1)) {
            s.spawn(|| {
                let mut decoder = dicts.decoder();
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some(path) = paths.get(i) else { break };
                    let got = file_programs(romfs, path, &mut decoder);
                    *results[i].lock().unwrap_or_else(|p| p.into_inner()) = Some(got);
                    progress(done.fetch_add(1, Ordering::SeqCst) as f64 / paths.len() as f64);
                }
            });
        }
    });
    let mut out = Census::default();
    let mut index: HashMap<u64, u32> = HashMap::new();
    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    for slot in results {
        let (found, skipped) = slot.into_inner().unwrap_or_else(|p| p.into_inner()).unwrap_or_default();
        out.skipped.extend(skipped);
        // (each of the file's stages hashed once, the first time a program names it: the list's order as before)
        let mut global: Vec<Option<u32>> = vec![None; found.stages.len()];
        let mut stages: Vec<Option<Stage>> = found.stages.into_iter().map(Some).collect();
        for local in found.programs {
            let mut ids = Vec::with_capacity(local.len());
            for l in local {
                let l = l as usize;
                if let Some(id) = global[l] {
                    ids.push(id);
                    continue;
                }
                let (stage, mut control, mut code) = stages[l].take().expect("a stage named once");
                // (what the back end reads of a stage: the 0xd8 control bytes and the code area they size)
                let end = u32::from_le_bytes(control[0x44..0x48].try_into().expect("a control section")) as usize;
                control.truncate(0xd8);
                if end <= code.len() {
                    code.truncate(end);
                }
                let mut h = sha1_smol::Sha1::new();
                h.update(&control);
                h.update(&code);
                let key = u64::from_le_bytes(h.digest().bytes()[..8].try_into().expect("8 bytes"));
                let id = *index.entry(key).or_insert_with(|| {
                    out.stages.push((key, stage, control, code));
                    (out.stages.len() - 1) as u32
                });
                global[l] = Some(id);
                ids.push(id);
            }
            ids.sort_unstable();
            ids.dedup();
            if seen.insert(ids.clone()) {
                out.programs.push(ids);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(stage: u16, code_bytes: u32) -> Vec<u8> {
        let mut c = vec![0u8; 0xd8];
        c[..4].copy_from_slice(&CONTROL_MAGIC);
        c[0x44..0x48].copy_from_slice(&code_bytes.to_le_bytes());
        c[0x56..0x58].copy_from_slice(&stage.to_le_bytes());
        c
    }

    /// An NDSR of a vertex and a fragment stage and a compute stage.
    fn ndsr() -> Vec<u8> {
        let mut d = b"NDSR".to_vec();
        d.resize(0x0c, 0);
        d.extend_from_slice(&6u32.to_le_bytes());
        d.resize(0x14 + 8 * 6, 0);
        let mut items = Vec::new();
        for (ty, stage, code) in [(0u32, 0u16, b"vvvv"), (3, 1, b"ffff"), (6, 5, b"cccc")] {
            let c = d.len();
            d.extend_from_slice(&control(stage, 4));
            let x = d.len();
            d.extend_from_slice(code);
            items.push((c as u32, ty));
            items.push((x as u32, ty + 1));
        }
        for (i, (at, ty)) in items.into_iter().enumerate() {
            d[0x14 + 8 * i..0x18 + 8 * i].copy_from_slice(&at.to_le_bytes());
            d[0x18 + 8 * i..0x1c + 8 * i].copy_from_slice(&(4 << 4 | ty).to_le_bytes());
        }
        d
    }

    #[test]
    fn an_ndsr_holds_a_graphics_program_and_a_compute_one() {
        let mut f = Found::default();
        programs_in(&ndsr(), &mut f).unwrap();
        let p: Vec<Vec<&Stage>> =
            f.programs.iter().map(|ids| ids.iter().map(|&i| &f.stages[i as usize]).collect()).collect();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].len(), 1, "the compute stage comes first, alone");
        assert_eq!(p[0][0].2, b"cccc");
        assert_eq!(p[1].iter().map(|s| s.0).collect::<Vec<_>>(), [0, 1]);
    }

    /// A BNSH of `variations` variations, each naming one program whose nine stages are all one stage.
    fn bnsh_of_one_stage(variations: usize) -> Vec<u8> {
        let (vars, program, slot) = (0x100, 0x100 + 64 * variations, 0x100 + 64 * variations + 80);
        let control_at = slot + 32;
        let code_at = control_at + 0xd8;
        let size = code_at + 4;
        let mut d = vec![0u8; size];
        d[..4].copy_from_slice(b"BNSH");
        d[12..14].copy_from_slice(&[0xFF, 0xFE]);
        d[28..32].copy_from_slice(&(size as u32).to_le_bytes());
        d[96..100].copy_from_slice(b"grsc");
        d[124..128].copy_from_slice(&(variations as u32).to_le_bytes());
        d[128..136].copy_from_slice(&(vars as u64).to_le_bytes());
        for i in 0..variations {
            d[vars + i * 64 + 16..vars + i * 64 + 24].copy_from_slice(&(program as u64).to_le_bytes());
        }
        for rel in SLOTS {
            d[program + rel..program + rel + 8].copy_from_slice(&(slot as u64).to_le_bytes());
        }
        d[slot + 8..slot + 16].copy_from_slice(&(control_at as u64).to_le_bytes());
        d[slot + 16..slot + 24].copy_from_slice(&(code_at as u64).to_le_bytes());
        d[slot + 24..slot + 28].copy_from_slice(&4u32.to_le_bytes());
        d[slot + 28..slot + 32].copy_from_slice(&0xd8u32.to_le_bytes());
        d[control_at..control_at + 0xd8].copy_from_slice(&control(1, 4));
        d[code_at..code_at + 4].copy_from_slice(b"code");
        d
    }

    #[test]
    fn a_stage_named_many_times_is_one_copy() {
        let mut f = Found::default();
        programs_in(&bnsh_of_one_stage(2000), &mut f).unwrap();
        assert_eq!(f.programs.len(), 2000);
        assert_eq!(f.stages.len(), 1, "2000 variations x 9 slots of one stage: one copy, not 18,000");
    }

    #[test]
    fn a_bahs_head_of_no_bytes_is_refused_not_walked_four_billion_times() {
        let mut d = b"BAHS".to_vec();
        d.resize(0x200, 0);
        // (no stages; the program table at data +0x150 + 0x20: 0xffffffff heads, each of 0 bytes)
        d[0x150..0x154].copy_from_slice(&0x20u32.to_le_bytes());
        d[0x174..0x178].copy_from_slice(&u32::MAX.to_le_bytes());
        let started = std::time::Instant::now();
        assert!(programs_in(&d, &mut Found::default()).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn a_sarc_holding_itself_is_refused() {
        // (one file, from the archive's first byte to its last: the archive itself)
        let mut d = b"SARC".to_vec();
        d.resize(0x40, 0);
        d[0x1a..0x1c].copy_from_slice(&1u16.to_le_bytes());
        d[0x2c..0x30].copy_from_slice(&0x40u32.to_le_bytes());
        let mut budget = FILE_BUDGET;
        let mut seen = 0;
        let got = blobs(d, &mut Decoder::plain(), 0, &mut budget, &mut |_| seen += 1);
        assert!(got.is_err() && seen == 0, "{got:?}");
    }

    #[test]
    fn sarcs_nest_three_deep_at_most_and_a_file_unpacks_to_its_budget() {
        let nest = |inner: Vec<u8>| crate::sarc::tests::build(&[("x", &inner)]);
        let mut d = b"leaf".to_vec();
        for _ in 0..4 {
            d = nest(d);
        }
        let mut budget = FILE_BUDGET;
        assert!(blobs(d.clone(), &mut Decoder::plain(), 0, &mut budget, &mut |_| {}).is_err(), "four deep");
        let mut tiny = 16;
        assert!(blobs(nest(vec![0; 64]), &mut Decoder::plain(), 0, &mut tiny, &mut |_| {}).is_err(), "past the budget");
    }

    #[test]
    fn the_pairs_found_in_one_pass_are_find_all_s() {
        let mut d = Vec::new();
        for i in 0..4000u32 {
            d.extend_from_slice(&(i % 7).to_le_bytes());
            d.extend_from_slice(&[0; 4][..(i % 3) as usize * 2]);
        }
        let wanted: HashSet<u32> = (0..9).collect();
        let pairs = pairs_of(&d, wanted.clone());
        for w in wanted {
            let mut pair = w.to_le_bytes().to_vec();
            pair.extend_from_slice(&[0; 4]);
            assert_eq!(pairs.get(&w).cloned().unwrap_or_default(), find_all(&d, &pair), "{w}");
        }
    }

    #[test]
    fn the_same_stage_twice_is_one_and_the_list_is_the_same_each_time() {
        let d = std::env::temp_dir().join(format!("build-assets-census-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Lib/agl")).unwrap();
        std::fs::create_dir_all(d.join("Pack")).unwrap();
        std::fs::write(d.join("Pack/ZsDic.pack.zs"), crate::zs::compress(&crate::sarc::tests::build(&[]))).unwrap();
        std::fs::write(d.join("Lib/agl/a.pck"), ndsr()).unwrap();
        std::fs::write(d.join("Lib/agl/b.pck.zs"), crate::zs::compress(&ndsr())).unwrap();
        std::fs::write(d.join("Lib/agl/c.pck"), b"BAHSshort").unwrap();
        let romfs = crate::romfs::Folder::new(&d).unwrap();
        let one = census(&romfs, 1, &|_| {}).unwrap();
        let three = census(&romfs, 3, &|_| {}).unwrap();
        assert_eq!(one, three);
        assert_eq!(one.stages.len(), 3);
        assert_eq!(one.programs.len(), 2);
        assert_eq!(one.skipped.len(), 1, "the BAHS cut short");
        let bytes = one.bytes();
        assert!(bytes.starts_with(b"HKSEED01"));
        assert_eq!(&bytes[8..16], &[2, 0, 0, 0, 3, 0, 0, 0]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_folder_s_files_come_before_its_folders() {
        let mut v = vec!["Lib/a/x.zs", "Lib/b.zs", "Lib/a.zs", "Lib/c/d/e.zs", "Lib/c/a.zs"];
        v.sort_by(|a, b| walk_order(a, b));
        assert_eq!(v, ["Lib/a.zs", "Lib/b.zs", "Lib/a/x.zs", "Lib/c/a.zs", "Lib/c/d/e.zs"]);
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find_all(&[0; 9], &[0; 4]), [0, 4]);
        assert_eq!(find_all(b"abcabc", b"bc"), [1, 4]);
    }
}

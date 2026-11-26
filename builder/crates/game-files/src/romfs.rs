//! The game's romfs read in place from the player's .nup: the index the builder writes into the game folder, in place
//! of the 26 GB the old builder unpacked there.
//!
//! The game (host/romfs.c over host/romfs_index.c) and the builder's own later stages ([`Index`]) read the romfs's
//! files through it, from the .nup itself: a file the .nup stores as it is (most of them) at its offset; one it keeps
//! as a zstd frame (40,588 of 186,587 in one dump, 2.75 GB of frames for 2.99 GB, none over 117 MB; none in another) is
//! decoded when it is opened. Those frames are the .nup's own wrapping of the game's `.zs` files, nearly all raw
//! blocks: libzstd decodes them at about 2 GB/s (the 117 MB one in 30 ms), so a game folder holds the index and
//! nothing of the romfs. The build reads the first bytes of each `.zs` file (to find the damaged ones), where an
//! unpack wrote 24 GB (more than the 23.3 GB .nup: files share data there).
//!
//! Read in place, not packed again: the .nup is already one file with every romfs file whole, so a pack would be a
//! second 26 GB. The index names the .nup by its path and identifies it by its size and its TOC's SHA-256 (the .nup's
//! header holds that hash, so the check reads 128 bytes): a .nup moved, deleted or replaced is said as that when the
//! game starts, and building again (from the .nup where it now is) writes a new index.
//!
//! # The index (`longshot_romfs.idx`, version 1, little-endian)
//!
//! ```text
//! 0x00  "LSRI"
//! 0x04  u32 version (1)
//! 0x08  u32 entries
//! 0x0c  u32 flags (0)
//! 0x10  u64 the .nup's size
//! 0x18  [32] the .nup's TOC SHA-256 (its header's, at +0x18)
//! 0x38  u32 the .nup's path's length in bytes
//! 0x3c  u32 the names' size in bytes
//! 0x40  the .nup's path (UTF-8, absolute), then zeros to a multiple of 8
//! then  entries, 32 bytes each, sorted by name (byte order):
//!         u64 offset of the data in the .nup, u64 the file's size, u64 the data's size in the .nup,
//!         u32 name offset (in the names), u16 name length, u8 codec (0: the file as it is; 1: one zstd frame of
//!         the data's size whose content is the file), u8 0
//! then  the names: each file's path under the romfs, "/" between its parts, UTF-8, not 0-terminated
//! ```
//!
//! Folders are not listed: a folder is the common start of the names under it (sorted, they are together). An
//! incomplete dump's damaged files point at their stand-ins' data ([`stand_ins`]).

use crate::le::{le32, le64};
use crate::nup::{Nup, Part, Title, read_at};
use crate::zstd::starts_as_zstd;
use build_core::{Error, IoContext, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

/// The index's file in the game folder.
pub const INDEX_FILE: &str = "longshot_romfs.idx";
/// The most a zstd-stored file of the index may decode to (host/romfs_index.c's limit: such a file is decoded whole).
pub const MAX_PACKED: u64 = 1 << 30;
const MAGIC: &[u8; 4] = b"LSRI";
const VERSION: u32 = 1;
const HEADER: usize = 0x40;
const ENTRY: usize = 32;

/// One file of the romfs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its path under the romfs ("/").
    pub name: String,
    /// Where its data is in the .nup.
    pub offset: u64,
    /// The file's size.
    pub size: u64,
    /// The data's size in the .nup.
    pub stored: u64,
    /// 0: the file as it is; 1: a zstd frame of `stored` bytes whose content is the file.
    pub codec: u8,
}

/// An incomplete dump's damaged file and what is read in its place.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StandIn {
    /// The damaged file (under the romfs).
    pub damaged: String,
    /// The whole file of the same kind read in its place, if one was found.
    pub with: Option<String>,
}

/// More damaged files than this and the dump is damaged throughout: none is given a stand-in, and the build stops at
/// once rather than search for hours (an incomplete dump has a few: three textures in the 23.3 GB dumps).
pub const MOST_STAND_INS: usize = 100;

/// A title's romfs as entries: each file's extent in the .nup, and how it is stored. A file kept as an unmarked frame
/// (older dumpers) is decoded once here to find its frame's end. Every name is a safe path (the assets write files by
/// them, the game opens them): a package with any other is refused. `progress(done, total)`.
pub fn entries(nup: &Nup, t: &Title, progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<Entry>> {
    let mut files = nup.part(t, Part::Romfs);
    for (name, _) in &files {
        crate::nup::package_parts(name)?;
    }
    // (probed in the .nup's order: each file's first bytes, read one after the other, not all over the disk)
    files.sort_by_key(|(_, i)| nup.files[*i].offset);
    let mut out = Vec::with_capacity(files.len());
    for (k, (name, i)) in files.iter().enumerate() {
        let r = &nup.files[*i];
        let (codec, stored) = if r.size == 0 {
            (0, 0)
        } else if r.codec == 1 {
            (1, r.stored)
        } else if nup.is_packed(r, name)? {
            (1, nup.frame_length(*i)?)
        } else {
            (0, r.size)
        };
        out.push(Entry { name: name.clone(), offset: r.offset, size: r.size, stored, codec });
        progress(k + 1, files.len());
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The `.zs` files among `entries` that do not start as zstd data (the 23.3 GB dumps have three that start with zeros,
/// and the game leaves out every model that uses one: the Water Temple's walls and floors). Each file's first bytes
/// only are read, in the .nup's order. `progress(done, total)`.
pub fn damaged(nup: &Nup, entries: &[Entry], progress: &mut dyn FnMut(usize, usize)) -> Result<Vec<String>> {
    let mut zs: Vec<&Entry> = entries.iter().filter(|e| e.name.ends_with(".zs")).collect();
    zs.sort_by_key(|e| e.offset);
    let mut out = Vec::new();
    for (k, e) in zs.iter().enumerate() {
        if !starts_as_zstd(&head(nup, e, 4)?) {
            out.push(e.name.clone());
        }
        progress(k + 1, zs.len());
    }
    out.sort();
    Ok(out)
}

/// The first `n` bytes of an entry's file.
fn head(nup: &Nup, e: &Entry, n: usize) -> Result<Vec<u8>> {
    if e.codec == 0 {
        let mut b = vec![0u8; n.min(e.size as usize)];
        let got = read_at(nup.file(), &mut b, e.offset)
            .map_err(|err| crate::nup::unreadable(nup.path(), e.offset, Some(nup.len), &err))?;
        b.truncate(got);
        return Ok(b);
    }
    nup.frame_head(e.offset, e.stored, n)
}

/// The suffix a stand-in must share: the last `_Kind` part and the extension (`_Nrm.bntx.zs`: a normal map for a
/// normal map).
fn kind_suffix(name: &str) -> &str {
    // (_[A-Za-z]+\d*)?(\.[^_]*)$: the extension is from the first dot after the last underscore
    let ext_at = name.rfind('_').unwrap_or(0);
    let Some(dot) = name[ext_at..].find('.').map(|d| d + ext_at) else { return "" };
    if name[dot..].contains('_') {
        return &name[name.len()..];
    }
    let before = &name[..dot];
    if let Some(u) = before.rfind('_') {
        let part = &before[u + 1..];
        let letters = part.trim_end_matches(|c: char| c.is_ascii_digit());
        if !letters.is_empty() && letters.chars().all(|c| c.is_ascii_alphabetic()) {
            return &name[u..];
        }
    }
    &name[dot..]
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// The ranking of a folder's files as stand-ins for `name` (of `size` bytes): the same kind (its suffix), the
/// longest common start of the name, then the nearest size; ties to the first in the listing. Never one of `damaged`.
pub fn rank_stand_ins<'a>(
    name: &str,
    size: u64,
    listing: &'a [(String, u64)],
    damaged: &dyn Fn(&str) -> bool,
) -> Vec<&'a str> {
    let suffix = kind_suffix(name);
    let mut ranked: Vec<(usize, u64, usize, &str)> = listing
        .iter()
        .enumerate()
        .filter(|(_, (o, _))| o != name && o.ends_with(suffix) && !damaged(o))
        .map(|(i, (o, s))| (usize::MAX - common_prefix(name, o), s.abs_diff(size), i, o.as_str()))
        .collect();
    ranked.sort();
    ranked.into_iter().map(|r| r.3).collect()
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// Each damaged file given the best whole file of its kind in its folder (opened in rank order until one starts as
/// zstd): its entry then points at that file's data. More than [`MOST_STAND_INS`]: none is given one.
pub fn stand_ins(nup: &Nup, entries: &mut [Entry], damaged: &[String]) -> Result<Vec<StandIn>> {
    if damaged.len() > MOST_STAND_INS {
        return Ok(damaged.iter().map(|d| StandIn { damaged: d.clone(), with: None }).collect());
    }
    let is_damaged = |n: &str| damaged.iter().any(|d| d == n);
    let mut out = Vec::new();
    for d in damaged {
        let (folder, name) = split(d);
        let listing: Vec<(String, u64)> = entries
            .iter()
            .filter(|e| split(&e.name).0 == folder)
            .map(|e| (split(&e.name).1.to_string(), e.size))
            .collect();
        let size = entries.iter().find(|e| &e.name == d).map_or(0, |e| e.size);
        let full = |n: &str| if folder.is_empty() { n.to_string() } else { format!("{folder}/{n}") };
        let mut with = None;
        for other in rank_stand_ins(name, size, &listing, &|n| is_damaged(&full(n))) {
            let o = full(other);
            let e = entries.iter().find(|e| e.name == o).cloned();
            if let Some(e) = e
                && starts_as_zstd(&head(nup, &e, 4)?)
            {
                if let Some(target) = entries.iter_mut().find(|t| &t.name == d) {
                    (target.offset, target.size, target.stored, target.codec) = (e.offset, e.size, e.stored, e.codec);
                }
                with = Some(o);
                break;
            }
        }
        out.push(StandIn { damaged: d.clone(), with });
    }
    Ok(out)
}

/// The longest .nup path an index holds, in bytes: host/romfs_index.c keeps it in a buffer of host/romfs.h's
/// `HOS_ROMFS_PATH` (1024) with its terminating 0, and refuses an index of a longer one.
pub const MAX_NUP_PATH: usize = 1023;

/// Why the game would refuse an index of these entries (sorted by name) of a .nup of `nup_len` bytes, if it would:
/// host/romfs_index.c's `entries_sane`, check for check, so the builder refuses at build time what the game would
/// refuse at its start (a change to one is a change to both: `the_host_s_reader` feeds them the same indexes).
fn refused(entries: &[Entry], nup_len: u64) -> Option<String> {
    let mut prev: Option<&str> = None;
    for e in entries {
        if e.name.is_empty() || e.name.len() > usize::from(u16::MAX) || e.codec > 1 {
            return Some(format!("an entry of no name, too long a name or an unknown codec ({:?})", e.name));
        }
        // (a zstd-stored file is decoded whole in memory: at most 1 GB, its frame at most a little larger than its
        // content, raw blocks of 128 KB taking 3 bytes each and the header 18)
        if e.codec == 1 && (e.size > MAX_PACKED || e.stored > e.size + (e.size >> 15) + 64) {
            return Some(format!("{}: a zstd-stored file over 1 GB, or its frame too large", e.name));
        }
        // (every file's data inside the .nup, one at offset 0 too)
        let extent = if e.codec == 1 { e.stored } else { e.size };
        if extent > nup_len || e.offset > nup_len - extent {
            return Some(format!("{}: its data runs past the .nup's end", e.name));
        }
        if prev.is_some_and(|p| p.as_bytes() >= e.name.as_bytes()) {
            return Some(format!("{}: the names are not in strict order", e.name));
        }
        prev = Some(&e.name);
    }
    // (files of data at one offset are one file's: a stand-in shares its frame whole. Two of other sizes, stored sizes
    // or codecs there would read one's bytes as the other's. Empty files read nothing and may be anywhere)
    let mut by_offset: Vec<&Entry> = entries.iter().filter(|e| e.size != 0).collect();
    by_offset.sort_by_key(|e| e.offset);
    by_offset
        .windows(2)
        .find(|w| {
            w[0].offset == w[1].offset && (w[0].size, w[0].stored, w[0].codec) != (w[1].size, w[1].stored, w[1].codec)
        })
        .map(|w| format!("{} and {} share their data but not its size", w[0].name, w[1].name))
}

/// The index's bytes for these entries (sorted by name) of the .nup at `nup_path`, refused when the game would refuse
/// them: a path too long for it (`nup_path_too_long`), files it could not read as they are laid out (`nup_damaged`,
/// [`refused`]).
pub fn encode(nup: &Nup, nup_path: &Path, entries: &[Entry]) -> Result<Vec<u8>> {
    let path = nup_path.to_str().ok_or_else(|| {
        Error::player(
            "nup_path_not_unicode",
            format!(
                "The .nup's path {} is not valid Unicode: move it to a folder with a plain name.",
                nup_path.display()
            ),
        )
    })?;
    if path.len() > MAX_NUP_PATH {
        return Err(Error::player(
            "nup_path_too_long",
            format!(
                "The .nup's path {} is {} bytes long, more than the game takes ({MAX_NUP_PATH}): move it to a folder \
                 with a shorter path.",
                nup_path.display(),
                path.len()
            ),
        ));
    }
    if let Some(why) = refused(entries, nup.len) {
        return Err(Error::player(
            "nup_damaged",
            format!(
                "The .nup {} lists its files in a way the game can't read ({why}): copy it again from the dumper, or \
                 dump the game again.",
                nup_path.display()
            ),
        ));
    }
    Ok(write_index(nup.len, &nup.toc_sha256, path, entries))
}

/// An index's bytes as they are, checked or not (the tests make damaged ones with it).
fn write_index(nup_len: u64, toc_sha256: &[u8; 32], path: &str, entries: &[Entry]) -> Vec<u8> {
    let mut names = Vec::new();
    let mut table = Vec::with_capacity(entries.len() * ENTRY);
    for e in entries {
        table.extend_from_slice(&e.offset.to_le_bytes());
        table.extend_from_slice(&e.size.to_le_bytes());
        table.extend_from_slice(&e.stored.to_le_bytes());
        table.extend_from_slice(&(names.len() as u32).to_le_bytes());
        table.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        table.extend_from_slice(&[e.codec, 0]);
        names.extend_from_slice(e.name.as_bytes());
    }
    let mut out = Vec::with_capacity(HEADER + path.len() + 8 + table.len() + names.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&nup_len.to_le_bytes());
    out.extend_from_slice(toc_sha256);
    out.extend_from_slice(&(path.len() as u32).to_le_bytes());
    out.extend_from_slice(&(names.len() as u32).to_le_bytes());
    out.extend_from_slice(path.as_bytes());
    out.resize(out.len().div_ceil(8) * 8, 0);
    out.extend_from_slice(&table);
    out.extend_from_slice(&names);
    out
}

/// What [`index`] made.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Indexed {
    /// The files.
    pub entries: usize,
    /// Their bytes.
    pub bytes: u64,
    /// The damaged files and their stand-ins.
    pub stand_ins: Vec<StandIn>,
}

/// The title's romfs indexed into `game/longshot_romfs.idx` (written whole: an index is complete or not there). A
/// damaged file with no stand-in is the caller's to refuse (the index is still written: the game reads the stand-ins it
/// has). `progress(0..1)`.
pub fn index(nup: &Nup, t: &Title, game: &Path, progress: &mut dyn FnMut(f64)) -> Result<Indexed> {
    let mut list = entries(nup, t, &mut |d, n| progress(0.5 * d as f64 / n.max(1) as f64))?;
    let bad = damaged(nup, &list, &mut |d, n| progress(0.5 + 0.5 * d as f64 / n.max(1) as f64))?;
    let found = stand_ins(nup, &mut list, &bad)?;
    let path = std::path::absolute(nup.path()).at(nup.path())?;
    build_core::fsx::write_atomic(&game.join(INDEX_FILE), &encode(nup, &path, &list)?)?;
    Ok(Indexed { entries: list.len(), bytes: list.iter().map(|e| e.size).sum(), stand_ins: found })
}

/// Beside the index: what [`index`] made it with ([`RECIPE`]) and found ([`Indexed`]), written after the index and
/// removed before it is written again, so an index with its note is one [`index_or_keep`] may keep.
pub const INDEX_NOTE: &str = ".longshot_romfs.json";

/// The rules an index is made by: [`VERSION`] (raised by hand for a change of format) and the hash of the sources that
/// decide what it holds (game-files' build.rs: romfs.rs, nup.rs, zstd.rs, le.rs), so an index made by other code is
/// made again.
pub const RECIPE: &str = concat!("romfs index 1, sources ", env!("ROMFS_SOURCES"));

#[derive(serde::Serialize, serde::Deserialize)]
struct Note {
    recipe: String,
    made: Indexed,
}

/// The game folder's index kept when it was made by these rules from this very .nup (its path, size and TOC hash:
/// [`Index::open`]), else made again ([`index`]); what it holds, and whether it was kept. A build reads the .nup's
/// 186 000 file heads to make one: 6 s from a cold disk cache, for an update that changes nothing.
pub fn index_or_keep(nup: &Nup, t: &Title, game: &Path, progress: &mut dyn FnMut(f64)) -> Result<(Indexed, bool)> {
    let kept = (|| {
        let note: Note = serde_json::from_slice(&std::fs::read(game.join(INDEX_NOTE)).ok()?).ok()?;
        let idx = Index::open(&game.join(INDEX_FILE)).ok()?;
        let ours = std::path::absolute(nup.path()).ok()?;
        (note.recipe == RECIPE && idx.nup() == ours && idx.entries().len() == note.made.entries).then_some(note.made)
    })();
    if let Some(made) = kept {
        return Ok((made, true));
    }
    build_core::fsx::remove_file(&game.join(INDEX_NOTE))?;
    let made = index(nup, t, game, progress)?;
    let note = serde_json::to_vec(&Note { recipe: RECIPE.into(), made: made.clone() })
        .map_err(|e| Error::format(INDEX_NOTE, e.to_string()))?;
    build_core::fsx::write_atomic(&game.join(INDEX_NOTE), &note)?;
    Ok((made, false))
}

/// The index of another game folder (`from`) used for this one (`game`) when it reads this very .nup (its path, size
/// and TOC hash): copied (16 MB), so a second game folder from one .nup (another version of the builder, another
/// configuration) indexes nothing. Whether it was; an index of another .nup, or none, is left alone.
pub fn reuse(from: &Path, game: &Path, nup: &Nup) -> Result<bool> {
    let theirs = from.join(INDEX_FILE);
    let Ok(other) = Index::open(&theirs) else { return Ok(false) };
    let ours = std::path::absolute(nup.path()).at(nup.path())?;
    if other.nup() != ours {
        return Ok(false);
    }
    if std::fs::canonicalize(from).ok() != std::fs::canonicalize(game).ok() {
        build_core::fsx::remove_file(&game.join(INDEX_NOTE))?;
        build_core::fsx::write_atomic(&game.join(INDEX_FILE), &std::fs::read(&theirs).at(&theirs)?)?;
        if let Ok(note) = std::fs::read(from.join(INDEX_NOTE)) {
            build_core::fsx::write_atomic(&game.join(INDEX_NOTE), &note)?;
        }
    }
    Ok(true)
}

/// The .nup an index at `path` was made from, as its header names it: its size and its TOC's SHA-256 (the .nup itself
/// not opened: it may have moved). `None`: no index of this builder there.
pub fn identity(path: &Path) -> Option<(u64, [u8; 32])> {
    let raw = std::fs::read(path).ok()?;
    if raw.len() < HEADER || &raw[..4] != MAGIC || le32(&raw, 4) != VERSION {
        return None;
    }
    Some((le64(&raw, 0x10), raw[0x18..0x38].try_into().ok()?))
}

/// An index read back: the builder's later stages read the romfs through it.
#[derive(Debug)]
pub struct Index {
    nup: PathBuf,
    file: File,
    entries: Vec<Entry>,
}

impl Index {
    /// The index at `path`, its .nup opened and checked to be the one it was made from.
    pub fn open(path: &Path) -> Result<Index> {
        let raw = std::fs::read(path).at(path)?;
        let bad = || Error::format(path.display().to_string(), "not a romfs index of this builder");
        if raw.len() < HEADER || &raw[..4] != MAGIC || le32(&raw, 4) != VERSION {
            return Err(bad());
        }
        let (count, path_len, names_len) =
            (le32(&raw, 8) as usize, le32(&raw, 0x38) as usize, le32(&raw, 0x3c) as usize);
        let table_at = (HEADER + path_len).div_ceil(8) * 8;
        let names_at = table_at + count * ENTRY;
        if path_len > MAX_NUP_PATH || raw.len() != names_at + names_len {
            return Err(bad());
        }
        let nup = PathBuf::from(std::str::from_utf8(&raw[HEADER..HEADER + path_len]).map_err(|_| bad())?);
        let mut entries = Vec::with_capacity(count);
        for k in 0..count {
            let e = &raw[table_at + k * ENTRY..table_at + (k + 1) * ENTRY];
            let (no, nl) = (le32(e, 24) as usize, u16::from_le_bytes([e[28], e[29]]) as usize);
            let name =
                raw.get(names_at + no..names_at + no + nl).and_then(|n| std::str::from_utf8(n).ok()).ok_or_else(bad)?;
            if crate::nup::package_parts(name).is_err() || e[31] != 0 {
                return Err(bad());
            }
            entries.push(Entry {
                name: name.into(),
                offset: le64(e, 0),
                size: le64(e, 8),
                stored: le64(e, 16),
                codec: e[30],
            });
        }
        // (what the game refuses, refused here, before the .nup is opened as the game does: host/romfs_index.c)
        if refused(&entries, le64(&raw, 0x10)).is_some() {
            return Err(bad());
        }
        let gone = |why: String| {
            Error::player(
                "nup_moved",
                format!(
                    "The game reads its files from your .nup {}, which {why}. Build again from the .nup where it is now, \
                     or, for the same .nup moved, point the game folder at it: longshot-builder index --game <the .nup> \
                     --out <the game folder>.",
                    nup.display()
                ),
            )
        };
        let file = File::open(&nup).map_err(|_| gone("is not there".into()))?;
        let mut head = [0u8; 0x80];
        let n = read_at(&file, &mut head, 0).at(&nup)?;
        let len = file.metadata().at(&nup)?.len();
        if n < 0x80 || len != le64(&raw, 0x10) || head[24..56] != raw[0x18..0x38] {
            return Err(gone("is not the one the game was built from".into()));
        }
        Ok(Index { nup, file, entries })
    }

    /// The .nup it reads.
    pub fn nup(&self) -> &Path {
        &self.nup
    }

    /// Every file, by name.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The file `rel` names, if there is one.
    pub fn find(&self, rel: &str) -> Option<&Entry> {
        self.entries.binary_search_by(|e| e.name.as_str().cmp(rel)).ok().map(|i| &self.entries[i])
    }

    /// Whether `rel` is a folder of the romfs.
    pub fn is_dir(&self, rel: &str) -> bool {
        let prefix = format!("{}/", rel.trim_end_matches('/'));
        let i = self.entries.partition_point(|e| e.name.as_str() < prefix.as_str());
        rel.is_empty() || self.entries.get(i).is_some_and(|e| e.name.starts_with(&prefix))
    }

    /// A whole file's bytes.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>> {
        let e = self.find(rel).ok_or_else(|| Error::format("romfs", format!("no file {rel}")))?;
        let mut data = vec![0u8; if e.codec == 1 { e.stored } else { e.size } as usize];
        let n = read_at(&self.file, &mut data, e.offset).at(&self.nup)?;
        if n != data.len() {
            return Err(Error::format(self.nup.display().to_string(), format!("{rel} is cut short")));
        }
        if e.codec == 0 {
            return Ok(data);
        }
        let mut out = Vec::with_capacity(e.size.min(64 << 20) as usize);
        let dec =
            ruzstd::decoding::StreamingDecoder::new(&data[..]).map_err(|err| Error::format(rel, err.to_string()))?;
        // (no more than one byte past its size decoded: a frame that would make more is refused, not followed)
        std::io::Read::read_to_end(&mut std::io::Read::take(dec, e.size + 1), &mut out)
            .map_err(|err| Error::format(rel, err.to_string()))?;
        if out.len() as u64 != e.size {
            return Err(Error::format(rel, "decoded to another size"));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nup::tests::{sample, scratch};

    #[test]
    fn a_kind_is_its_last_part_and_extension() {
        assert_eq!(kind_suffix("Wall_Alb.bntx.zs"), "_Alb.bntx.zs");
        assert_eq!(kind_suffix("Rock01_Nrm2.bntx.zs"), "_Nrm2.bntx.zs");
        assert_eq!(kind_suffix("plain.bntx.zs"), ".bntx.zs");
        assert_eq!(kind_suffix("x_1.zs"), ".zs");
        assert_eq!(kind_suffix("noext"), "");
    }

    #[test]
    fn stand_ins_rank_by_name_then_size() {
        let listing: Vec<(String, u64)> =
            [("Wall_Alb.bntx.zs", 10), ("Wall2_Alb.bntx.zs", 50), ("Wallx_Alb.bntx.zs", 12), ("Wall_Nrm.bntx.zs", 10)]
                .iter()
                .map(|(n, s)| (n.to_string(), *s))
                .collect();
        assert_eq!(
            rank_stand_ins("Wall_Alb.bntx.zs", 10, &listing, &|_| false),
            ["Wallx_Alb.bntx.zs", "Wall2_Alb.bntx.zs"]
        );
        assert_eq!(
            rank_stand_ins("Wall_Alb.bntx.zs", 10, &listing, &|n| n == "Wallx_Alb.bntx.zs"),
            ["Wall2_Alb.bntx.zs"]
        );
    }

    #[test]
    fn a_romfs_name_that_is_no_safe_path_refuses_the_package() {
        let d = scratch("romfs-unsafe");
        let p = d.join("game.nup");
        for bad in ["../outside.zs", "Pack/con.txt", "a:b"] {
            let mut s = sample();
            s.titles[1].3.push((bad.to_string(), b"x".to_vec(), 0));
            std::fs::write(&p, s.build()).unwrap();
            let Ok(nup) = Nup::open(&p) else { continue }; // (refused at open is as good)
            let t = nup.pick_title(None).unwrap().clone();
            let e = entries(&nup, &t, &mut |_, _| {});
            assert!(matches!(e, Err(Error::Player { code: "nup_unsafe_path", .. })), "{bad}: {e:?}");
            assert!(index(&nup, &t, &d, &mut |_| {}).is_err());
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    mod recipe_hash {
        include!(concat!(env!("CARGO_MANIFEST_DIR"), "/recipe_hash.rs"));
    }

    #[test]
    fn the_recipe_is_of_the_sources_that_make_an_index_and_changes_with_them() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let now = recipe_hash::hash(&src);
        assert!(RECIPE.ends_with(&now), "the recipe is the hash of these files: {RECIPE}");
        assert!(RECIPE.contains(&format!("romfs index {VERSION},")), "and the index's format version");
        // (each file a part of it: one byte changed in a copy, another hash)
        let d = scratch("romfs-recipe");
        for f in recipe_hash::FILES {
            std::fs::copy(src.join(f), d.join(f)).unwrap();
        }
        assert_eq!(recipe_hash::hash(&d), now);
        for f in recipe_hash::FILES {
            let mut bytes = std::fs::read(src.join(f)).unwrap();
            bytes.push(b'\n');
            std::fs::write(d.join(f), &bytes).unwrap();
            assert_ne!(recipe_hash::hash(&d), now, "{f} changed, the recipe did not");
            std::fs::copy(src.join(f), d.join(f)).unwrap();
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_index_made_from_this_nup_by_these_rules_is_kept() {
        let d = scratch("romfs-keep");
        let p = d.join("game.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        let t = nup.pick_title(None).unwrap().clone();
        let (made, kept) = index_or_keep(&nup, &t, &d, &mut |_| {}).unwrap();
        assert!(!kept);
        let mut read = false;
        let (again, kept) = index_or_keep(&nup, &t, &d, &mut |_| read = true).unwrap();
        assert!(kept && !read && again == made, "the same .nup: nothing read again");
        // (a note of other rules: made again)
        let note = std::fs::read_to_string(d.join(INDEX_NOTE)).unwrap();
        std::fs::write(d.join(INDEX_NOTE), note.replace(RECIPE, "romfs index 1, sources 0")).unwrap();
        assert!(!index_or_keep(&nup, &t, &d, &mut |_| {}).unwrap().1);
        // (another package at that path: made again from it)
        let mut other = sample();
        other.titles[1].3.pop();
        std::fs::write(&p, other.build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        let t = nup.pick_title(None).unwrap().clone();
        let (fresh, kept) = index_or_keep(&nup, &t, &d, &mut |_| {}).unwrap();
        assert!(!kept && fresh.entries + 1 == made.entries);
        // (no note, as an index written by a builder before this one: made again)
        std::fs::remove_file(d.join(INDEX_NOTE)).unwrap();
        assert!(!index_or_keep(&nup, &t, &d, &mut |_| {}).unwrap().1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_packed_entry_over_1_gb_or_of_too_big_a_frame_refuses_the_index() {
        let d = scratch("romfs-packed-cap");
        let p = d.join("game.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        let t = nup.pick_title(None).unwrap().clone();
        index(&nup, &t, &d, &mut |_| {}).unwrap();
        let good = std::fs::read(d.join(INDEX_FILE)).unwrap();
        let (count, path_len) = (le32(&good, 8) as usize, le32(&good, 0x38) as usize);
        let table_at = (HEADER + path_len).div_ceil(8) * 8;
        let k = (0..count).find(|k| good[table_at + k * ENTRY + 30] == 1).expect("a packed entry");
        let e = table_at + k * ENTRY;
        let size = le64(&good, e + 8);
        for (field, v) in [(8, MAX_PACKED + 1), (16, size + (size >> 15) + 65)] {
            let mut bad = good.clone();
            bad[e + field..e + field + 8].copy_from_slice(&v.to_le_bytes());
            std::fs::write(d.join(INDEX_FILE), &bad).unwrap();
            assert!(Index::open(&d.join(INDEX_FILE)).is_err(), "field {field} = {v}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_index_reads_the_romfs_from_the_package() {
        let d = scratch("romfs-index");
        let p = d.join("game.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        let t = nup.pick_title(None).unwrap().clone();
        let made = index(&nup, &t, &d, &mut |_| {}).unwrap();
        assert_eq!(
            made.stand_ins,
            [StandIn { damaged: "Model/Wall_Alb.bntx.zs".into(), with: Some("Model/Wall2_Alb.bntx.zs".into()) }]
        );
        let idx = Index::open(&d.join(INDEX_FILE)).unwrap();
        assert!(idx.read("big.bin").unwrap() == crate::testdata::text(), "a zstd-stored file decoded");
        assert_eq!(idx.read("Pack/ZsDic.pack.zs").unwrap()[..4], crate::zstd::MAGIC);
        assert_eq!(idx.read("Model/Wall_Alb.bntx.zs").unwrap(), idx.read("Model/Wall2_Alb.bntx.zs").unwrap());
        assert!(idx.is_dir("Pack") && idx.is_dir("System/Resource") && !idx.is_dir("Pac") && !idx.is_dir("big.bin"));
        assert_eq!(idx.read("System/Resource/empty.txt").unwrap().len(), 0);
        // another game folder from the same package takes this one's index
        let second = d.join("second");
        std::fs::create_dir_all(&second).unwrap();
        assert!(reuse(&d, &second, &nup).unwrap() && Index::open(&second.join(INDEX_FILE)).is_ok());
        assert!(!reuse(&d.join("none"), &second, &nup).unwrap());
        // another package at that path: said, not read
        let mut other = sample();
        other.titles[1].3.pop();
        std::fs::write(&p, other.build()).unwrap();
        assert!(matches!(Index::open(&d.join(INDEX_FILE)), Err(Error::Player { code: "nup_moved", .. })));
        std::fs::remove_file(&p).unwrap();
        assert!(matches!(Index::open(&d.join(INDEX_FILE)), Err(Error::Player { code: "nup_moved", .. })));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Indexes the game refuses (host/romfs_index.c), made from a good one's entries of the .nup `nup` at `path`:
    /// each one's name, its bytes, and the code the builder refuses it with at build time ([`encode`]).
    /// `every_index_the_game_refuses_the_builder_refuses` gives them to the builder, `the_host_s_reader` to the game.
    fn refused_indexes(nup: &Nup, path: &str, good: &[Entry]) -> Vec<(&'static str, Vec<u8>, &'static str)> {
        let bytes = |entries: &[Entry]| write_index(nup.len, &nup.toc_sha256, path, entries);
        let with = |f: &dyn Fn(&mut Vec<Entry>)| {
            let mut e = good.to_vec();
            f(&mut e);
            bytes(&e)
        };
        let raw = good.iter().position(|e| e.codec == 0 && e.size > 0).expect("a raw entry");
        let zstd: Vec<usize> = (0..good.len()).filter(|&k| good[k].codec == 1).collect();
        let (small, large) =
            if good[zstd[0]].stored <= good[zstd[1]].stored { (zstd[0], zstd[1]) } else { (zstd[1], zstd[0]) };
        assert_ne!((good[small].size, good[small].stored), (good[large].size, good[large].stored));
        let empty = good.iter().position(|e| e.size == 0).expect("an empty file");
        let len = nup.len;
        vec![
            // (an entry past the .nup's end)
            ("past", with(&|e| e[0].offset = u64::MAX / 4), "nup_damaged"),
            // (a raw one at offset 0 longer than the .nup: offset 0 is checked as any other)
            (
                "past_from_0",
                with(&|e| (e[raw].offset, e[raw].size, e[raw].stored) = (0, len + 1, len + 1)),
                "nup_damaged",
            ),
            // (the names out of order: a lookup is a binary search)
            ("unsorted", with(&|e| e.swap(0, 1)), "nup_damaged"),
            // (two zstd-stored entries at one offset, of other sizes: a read of the smaller's copy as the larger's
            // would run past it)
            ("shared", with(&|e| e[small].offset = e[large].offset), "nup_damaged"),
            // (and so with an empty file at that offset too, which the sort may put between them)
            (
                "shared_by_an_empty_file",
                with(&|e| {
                    e[small].offset = e[large].offset;
                    e[empty].offset = e[large].offset;
                }),
                "nup_damaged",
            ),
            ("over_1_gb", with(&|e| e[small].size = MAX_PACKED + 1), "nup_damaged"),
            ("too_big_a_frame", with(&|e| e[small].stored = e[small].size + (e[small].size >> 15) + 65), "nup_damaged"),
            ("unknown_codec", with(&|e| e[raw].codec = 2), "nup_damaged"),
            // (a .nup path of 1024 bytes: the game's buffer holds 1023 and the 0)
            (
                "path_too_long",
                write_index(nup.len, &nup.toc_sha256, &format!("/{}", "a".repeat(MAX_NUP_PATH)), good),
                "nup_path_too_long",
            ),
        ]
    }

    /// The entries of an index's bytes, as [`Index::open`] reads them, without the .nup's checks.
    fn entries_of(raw: &[u8]) -> (String, Vec<Entry>) {
        let (count, path_len) = (le32(raw, 8) as usize, le32(raw, 0x38) as usize);
        let table_at = (HEADER + path_len).div_ceil(8) * 8;
        let names_at = table_at + count * ENTRY;
        let path = String::from_utf8(raw[HEADER..HEADER + path_len].to_vec()).unwrap();
        let entries = (0..count)
            .map(|k| {
                let e = &raw[table_at + k * ENTRY..];
                let (no, nl) = (le32(e, 24) as usize, u16::from_le_bytes([e[28], e[29]]) as usize);
                Entry {
                    name: String::from_utf8(raw[names_at + no..names_at + no + nl].to_vec()).unwrap(),
                    offset: le64(e, 0),
                    size: le64(e, 8),
                    stored: le64(e, 16),
                    codec: e[30],
                }
            })
            .collect();
        (path, entries)
    }

    #[test]
    fn every_index_the_game_refuses_the_builder_refuses() {
        let d = scratch("romfs-refused");
        let p = d.join("game.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        let t = nup.pick_title(None).unwrap().clone();
        index(&nup, &t, &d, &mut |_| {}).unwrap();
        let (path, good) = entries_of(&std::fs::read(d.join(INDEX_FILE)).unwrap());
        assert_eq!(refused(&good, nup.len), None);
        // (a raw file at offset 0 inside the .nup is taken, as the game takes it)
        let mut at_0 = good.clone();
        let raw = at_0.iter().position(|e| e.codec == 0 && e.size > 0).unwrap();
        at_0[raw].offset = 0;
        assert_eq!(refused(&at_0, nup.len), None);
        for (name, bytes, code) in refused_indexes(&nup, &path, &good) {
            let bad = d.join(format!("{name}.idx"));
            std::fs::write(&bad, &bytes).unwrap();
            assert!(matches!(Index::open(&bad), Err(Error::Format { .. })), "{name}: read back, refused as damaged");
            let (path, entries) = entries_of(&bytes);
            let made = encode(&nup, Path::new(&path), &entries);
            assert!(matches!(&made, Err(Error::Player { code: c, .. }) if *c == code), "{name}: {made:?}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// host/romfs_index.c, built with the C compiler on the PATH (`cc`, or `CC`), reads the same bytes as the folder an
    /// extraction makes, from four threads (tests/romfs_index_test.c). Left out where there is no compiler.
    /// `sources` (from the repository) built into `exe` with `-Werror`, with the vendored zstd decoder as it is (no
    /// assembly): whether it built.
    #[cfg(unix)]
    fn build_c(cc: &str, repo: &Path, d: &Path, exe: &Path, sources: &[&str], defines: &[&str]) -> bool {
        let zstd = repo.join("third_party/zstd/lib");
        let mut objects = Vec::new();
        for (k, src) in [
            "common/entropy_common.c",
            "common/error_private.c",
            "common/fse_decompress.c",
            "common/xxhash.c",
            "common/zstd_common.c",
            "decompress/huf_decompress.c",
            "decompress/zstd_ddict.c",
            "decompress/zstd_decompress_block.c",
            "decompress/zstd_decompress.c",
        ]
        .iter()
        .enumerate()
        {
            let o = d.join(format!("zstd{k}.o"));
            if !o.exists() {
                let ok = std::process::Command::new(cc)
                    .args(["-c", "-O1", "-DZSTD_DISABLE_ASM"])
                    .arg(zstd.join(src))
                    .arg("-o")
                    .arg(&o)
                    .status()
                    .is_ok_and(|s| s.success());
                if !ok {
                    return false;
                }
            }
            objects.push(o);
        }
        std::process::Command::new(cc)
            .args(["-std=gnu11", "-O1", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .args(defines)
            .arg("-I")
            .arg(&zstd)
            .arg("-I")
            .arg(repo.join("host/posix"))
            .args(sources.iter().map(|s| repo.join(s)))
            .args(&objects)
            .arg("-o")
            .arg(exe)
            .status()
            .is_ok_and(|s| s.success())
    }

    /// host/zs.c streams a .zs file the .nup wraps in a frame of its own without decoding it whole
    /// (tests/zs_test.c). Left out where there is no compiler.
    #[cfg(unix)]
    #[test]
    fn the_host_streams_a_wrapped_zs() {
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        if std::process::Command::new(&cc).arg("--version").output().is_err() {
            eprintln!("the_host_streams_a_wrapped_zs: no C compiler ({cc}): left out");
            return;
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let d = scratch("romfs-zs");
        let p = d.join("game.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        let t = nup.pick_title(None).unwrap().clone();
        index(&nup, &t, &d, &mut |_| {}).unwrap();
        let exe = d.join("zs_test");
        assert!(build_c(
            &cc,
            &repo,
            &d,
            &exe,
            &["tests/zs_test.c", "host/romfs.c", "host/romfs_index.c", "host/prompts_base.c"],
            &["-DHOS_ZS_ALONE"]
        ));
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let run = std::process::Command::new(&exe)
            .arg(d.join(INDEX_FILE))
            .arg(data.join("text.dict"))
            .arg(data.join("text.dict.zst"))
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_host_s_reader() {
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        if std::process::Command::new(&cc).arg("--version").output().is_err() {
            eprintln!("the_host_s_reader: no C compiler ({cc}): left out");
            return;
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let d = scratch("romfs-host");
        let p = d.join("game.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        let t = nup.pick_title(None).unwrap().clone();
        let game = d.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let made = index(&nup, &t, &game, &mut |_| {}).unwrap();
        let reference = d.join("x");
        let got = crate::nup::extract(&nup, &t, &reference, Some(Part::Romfs), &mut |_| {}).unwrap();
        for d in &got.damaged {
            // (what the index reads in its place: its stand-in)
            let with = made.stand_ins.iter().find(|s| &s.damaged == d).and_then(|s| s.with.clone()).unwrap();
            std::fs::copy(reference.join("romfs").join(&with), reference.join("romfs").join(d)).unwrap();
        }
        let names: Vec<String> =
            Index::open(&game.join(INDEX_FILE)).unwrap().entries().iter().map(|e| e.name.clone()).collect();
        std::fs::write(d.join("list.txt"), names.join("\n") + "\n").unwrap();
        let exe = d.join("romfs_test");
        assert!(
            build_c(
                &cc,
                &repo,
                &d,
                &exe,
                &["tests/romfs_test.c", "host/zs.c", "host/romfs.c", "host/romfs_index.c", "host/prompts_base.c"],
                &["-DHOS_ZS_ALONE"]
            ),
            "the C reader builds"
        );
        let run = std::process::Command::new(&exe)
            .arg(game.join(INDEX_FILE))
            .arg(reference.join("romfs"))
            .arg(d.join("list.txt"))
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&run.stdout);
        assert!(run.status.success(), "{out}{}", String::from_utf8_lossy(&run.stderr));
        assert!(
            out.contains(&format!("ok {} files", names.len()))
                && out.contains("root: Case/ Model/ Pack/ System/ big.bin "),
            "{out}"
        );
        // every index the builder refuses (refused_indexes): refused as romfs_index_bad, before the .nup is opened
        let (path, good) = entries_of(&std::fs::read(game.join(INDEX_FILE)).unwrap());
        for (name, bytes, _) in refused_indexes(&nup, &path, &good) {
            let bad = d.join(format!("{name}.idx"));
            std::fs::write(&bad, bytes).unwrap();
            let run = std::process::Command::new(&exe).arg("bad").arg(&bad).output().unwrap();
            assert!(run.status.success(), "{name}: {}", String::from_utf8_lossy(&run.stdout));
        }
        // another .nup where the index's was: refused with its code
        let mut other = sample();
        other.titles[1].3.pop();
        std::fs::write(&p, other.build()).unwrap();
        let run = std::process::Command::new(&exe)
            .arg(game.join(INDEX_FILE))
            .arg(reference.join("romfs"))
            .arg(d.join("list.txt"))
            .output()
            .unwrap();
        assert!(!run.status.success() && String::from_utf8_lossy(&run.stderr).contains("nup_moved:"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}

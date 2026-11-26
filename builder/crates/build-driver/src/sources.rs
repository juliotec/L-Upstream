//! What a release's longshot-builder carries: everything a player's PC builds the game with. The sources the compile
//! reads (host/, gpu/, the third-party code and assets the build reads, the release's name), the prebuilt third-party
//! objects, and the build tools, as one pack appended to the program after it is linked (`longshot-builder dev release
//! sources` makes it, with [`write`] here; `dev release attach` appends it, with [`attach`]). A developer's build carries
//! none and builds from a checkout (`--root`, `LONGSHOT_ROOT`, or the checkout the program is in).
//!
//! The pack is made of members, each a zstd frame of its own, so that a release whose tools (or most of whose sources)
//! did not change has the same bytes for those members, and a delta from the last release (`zstd --patch-from`) is
//! small:
//! - `tree/<part>`: the build's tree ([`TREE`] as tracked, but [`LEFT_OUT`]; its parts by [`group`]).
//! - `tools/<name>`: a build tool, at `tools/<name>/` in the tree ([`tool_dir`]). Each is unpacked once into
//!   `<cache>/tools/<name>-<its hash>/` and hard-linked from there into the tree, so the next release that carries the
//!   same tool unpacks nothing of it.
//!
//! The tree is `<cache>/sources/<the members' hash>/`, the build's root ([`crate::layout::tree_root`]).
//!
//! Each folder is unpacked once, its members' and every file's SHA-256 checked, into a folder of its own, put in place
//! whole (a rename) once it is complete, its [`COMPLETE`] note written last. A command that finds its folders there
//! reads the pack's table alone (the tree's hash is of the members' recorded hashes): nothing of the pack's hundreds
//! of MB is hashed again. Each use marks its folders ([`USED`]): the cache's budget (crate::storage) lets the least
//! recently used go, never while a build runs.
//!
//! ```text
//! the pack:    PACK_MAGIC, u32 members (LE), then for each: u16 name length, name, u64 offset, u64 length (from the
//!              pack's start, LE), 32 bytes SHA-256 of the member's bytes; then the members' bytes
//! a member:    one zstd frame of: MAGIC, then for each file: u16 path length (LE), path (UTF-8, / separated,
//!              relative), u8 mode (0 plain, 1 executable, 2 a relative symbolic link inside the member, its target
//!              the bytes; Unix only), u64 size (LE), 32 bytes SHA-256, the bytes; then u16 0,
//!              u64 the number of files (LE); in a release's pack, then a zstd skippable frame of 32 bytes: the
//!              member's key ([`member_key`], [`key_frame`]), which every reader passes over
//! ```
//!
//! The program carries its pack at the end of its file, past everything its headers name (Linux: the ELF's segments,
//! sections and section headers; Windows: the PE's overlay, past its sections), then a trailer of [`TRAILER_LEN`]
//! bytes, the file's last ([`trailer`]):
//!
//! ```text
//! the trailer: TRAILER_MAGIC, u32 TRAILER_VERSION (LE), u32 0, u64 the pack's offset in the file, u64 its length (LE),
//!              32 bytes SHA-256 of the pack; the pack ends where the trailer starts
//! ```
//!
//! The running program maps its own file ([`load_carried`]) and reads its pack there, nothing read whole. A file with no
//! trailer and nothing past its image carries no pack (a developer's build). One with bytes past its image but no whole
//! trailer, or whose pack fails its SHA-256 (checked when something is first unpacked from it), is damaged, a download
//! cut short, and says so (`tools_incomplete`: download it again). The pack ends at the same place relative to the
//! file's end in every release, so a delta from the last release (`zstd --patch-from`) stays small.
//!
//! This file is the format's one writer ([`write`], [`tree_entries`], [`tool_entries`]) and its one reader ([`unpack`],
//! [`unpack_into`]). The writer compresses each member with the zstd program at [`MEMBER_ZSTD`] (one thread, the
//! frame's checksum), so the same files give the same bytes, and a release's members that did not change keep theirs.
//! A release's members are made several at once ([`write_with`], one zstd process each, put in the pack in their
//! order), and each ends with its key, the SHA-256 of its raw bytes' and of what compressed them (the zstd program's
//! version, [`MEMBER_ZSTD`]): a member of the last release's pack (its signature checked: `dev release last-objects`)
//! with this one's name and key, its frame decompressed to these raw bytes, is taken as it is, the bytes compressing
//! them again would give, and the rest are compressed.

use build_core::{Error, IoContext, Result, fsx};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The pack's first bytes, its format's version in them.
pub const PACK_MAGIC: &[u8; 8] = b"LSPAK02\n";
/// A member's first bytes (after its zstd frame is undone).
pub const MAGIC: &[u8; 8] = b"LSSRC01\n";
/// In an unpacked folder: written last, the hash it was unpacked from and its files.
pub const COMPLETE: &str = ".complete";
/// In an unpacked folder: touched by each use (its time is the folder's last use).
pub const USED: &str = ".used";

/// The trailer's first bytes, its format's version in them.
pub const TRAILER_MAGIC: &[u8; 8] = b"LSPKEND\n";
/// The trailer's version (after its magic).
pub const TRAILER_VERSION: u32 = 1;
/// The trailer's size: the last bytes of a program that carries a pack.
pub const TRAILER_LEN: usize = 64;

/// The pack the running program carries: its bytes, and the SHA-256 its trailer gives them.
static CARRIED: OnceLock<(&'static [u8], [u8; 32])> = OnceLock::new();
/// The carried pack's SHA-256 checked (once a process): why not, when it is wrong.
static CARRIED_CHECKED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
static TOOLS: Mutex<BTreeMap<String, PathBuf>> = Mutex::new(BTreeMap::new());

/// The pack the program carries; empty when it carries none (a developer's build, or before [`load_carried`]).
pub fn embedded() -> &'static [u8] {
    CARRIED.get().map_or(&[], |c| c.0)
}

/// The pack the running program carries, found at the end of its own file (std::env::current_exe), mapped, not read:
/// the longshot-builder binary calls this first thing. Ok with nothing carried for a program with no trailer and nothing
/// past its image (a developer's build); an error (`tools_incomplete`) for a file cut short or damaged.
pub fn load_carried() -> Result<()> {
    if CARRIED.get().is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| bad(format!("this program's file: {e}")))?;
    let file = std::fs::File::open(&exe).at(&exe)?;
    if file.metadata().at(&exe)?.len() < TRAILER_LEN as u64 {
        return Ok(());
    }
    let map: &'static memmap2::Mmap = Box::leak(Box::new(map_file(&file).at(&exe)?));
    if let Some((range, sha)) = carried_in(map)? {
        let _ = CARRIED.set((&map[range], sha));
    }
    Ok(())
}

/// `file` mapped read only. A program's own file: nothing writes it while it runs (Linux refuses, ETXTBSY; Windows
/// holds it open), and a self-update renames it, which a mapping outlives.
#[allow(unsafe_code)]
fn map_file(file: &std::fs::File) -> std::io::Result<memmap2::Mmap> {
    // SAFETY: read only, of the running program's own file, which nothing changes under it (above).
    unsafe { memmap2::Mmap::map(file) }
}

/// The carried pack's SHA-256 checked against its trailer's, once a process (it hashes the pack whole, so only when
/// something is unpacked from it). Ok when the program carries none.
fn check_carried() -> Result<()> {
    let Some((pack, want)) = CARRIED.get() else { return Ok(()) };
    CARRIED_CHECKED
        .get_or_init(|| if Sha256::digest(pack).as_slice() == want { Ok(()) } else { Err(DAMAGED.into()) })
        .clone()
        .map_err(bad)
}

/// Why a pack whose SHA-256 is wrong is refused.
const DAMAGED: &str = "its SHA-256 is not the one it was made with: the download is damaged";

/// The trailer of a pack of `pack`'s bytes at `offset` in the file.
pub fn trailer(offset: u64, pack: &[u8]) -> [u8; TRAILER_LEN] {
    let mut t = [0u8; TRAILER_LEN];
    t[..8].copy_from_slice(TRAILER_MAGIC);
    t[8..12].copy_from_slice(&TRAILER_VERSION.to_le_bytes());
    t[16..24].copy_from_slice(&offset.to_le_bytes());
    t[24..32].copy_from_slice(&(pack.len() as u64).to_le_bytes());
    t[32..].copy_from_slice(&Sha256::digest(pack));
    t
}

/// `program` (a linked longshot-builder that carries none) with `pack` appended and its trailer after it. Refused for a
/// program that carries something past its image already, or whose headers this does not read.
pub fn attach(program: &[u8], pack: &[u8]) -> std::result::Result<Vec<u8>, String> {
    if pack.get(..8) != Some(&PACK_MAGIC[..]) {
        return Err("not a pack of this format".into());
    }
    match image_end(program) {
        None => return Err("not a Linux (ELF) or Windows (PE) program this reads".into()),
        Some(end) if end != program.len() || has_trailer(program) => {
            return Err("it carries something past its image already: attach to a program that carries no pack".into());
        }
        Some(_) => {}
    }
    let mut out = Vec::with_capacity(program.len() + pack.len() + TRAILER_LEN);
    out.extend_from_slice(program);
    out.extend_from_slice(pack);
    out.extend_from_slice(&trailer(program.len() as u64, pack));
    Ok(out)
}

/// Whether `file` ends with a trailer's magic.
fn has_trailer(file: &[u8]) -> bool {
    file.len() >= TRAILER_LEN && file[file.len() - TRAILER_LEN..].starts_with(TRAILER_MAGIC)
}

/// Where in `file` (a program's bytes) its pack is, and the SHA-256 its trailer gives it: None when it carries none (no
/// trailer, nothing past its image). An error for a file cut short or damaged: bytes past its image but no trailer, a
/// trailer of another version, or one naming bytes the file has not. The pack's SHA-256 is not checked here
/// ([`verify`] does).
pub fn carried_in(file: &[u8]) -> Result<Option<(std::ops::Range<usize>, [u8; 32])>> {
    let cut = || bad("the program's file is cut short or damaged: a download that did not end");
    if !has_trailer(file) {
        return match image_end(file) {
            Some(end) if end != file.len() => Err(cut()),
            _ => Ok(None),
        };
    }
    let at = file.len() - TRAILER_LEN;
    let t = &file[at..];
    if u32::from_le_bytes(t[8..12].try_into().unwrap_or_default()) != TRAILER_VERSION {
        return Err(bad("its pack's trailer is of another version"));
    }
    let (off, n) = (le_u64(&t[16..24]), le_u64(&t[24..32]));
    let start = usize::try_from(off).map_err(|_| cut())?;
    if off.checked_add(n) != Some(at as u64) || image_end(file).is_some_and(|end| end > start) {
        return Err(cut());
    }
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&t[32..]);
    Ok(Some((start..at, sha)))
}

/// `file`'s carried pack checked whole: found ([`carried_in`]), its SHA-256 its trailer's, its table read. Its bytes.
pub fn verify(file: &[u8]) -> Result<&[u8]> {
    let (range, want) = carried_in(file)?.ok_or_else(|| bad("it carries no pack"))?;
    let pack = &file[range];
    if Sha256::digest(pack).as_slice() != want {
        return Err(bad(DAMAGED));
    }
    members(pack)?;
    Ok(pack)
}

/// The pack a release's program carries, wherever it is: after its trailer's check ([`carried_in`]) and its SHA-256's,
/// or, in a program from before the pack was appended, compiled into it (the first place a whole pack starts at, each
/// member inside it). None when it carries none.
pub fn find_pack(file: &[u8]) -> Option<&[u8]> {
    if has_trailer(file) {
        return verify(file).ok();
    }
    let mut from = 0;
    while let Some(i) = file[from..].windows(PACK_MAGIC.len()).position(|w| w == PACK_MAGIC) {
        let pack = &file[from + i..];
        if let Ok(all) = members(pack) {
            let end = all.iter().map(|m| m.bytes.as_ptr() as usize + m.bytes.len() - pack.as_ptr() as usize).max();
            return Some(&pack[..end.unwrap_or(0)]);
        }
        from += i + 1;
    }
    None
}

/// One member of `pack` written into `folder`, its SHA-256 and every file's checked: its files (paths in the tree, as
/// [`unpack_into`] writes them).
pub fn unpack_member(pack: &[u8], name: &str, folder: &Path) -> Result<u64> {
    let all = members(pack)?;
    let m = all.iter().find(|m| m.name == name).ok_or_else(|| bad(format!("no member {name}")))?;
    if hex(&Sha256::digest(m.bytes)) != m.sha {
        return Err(bad(format!("{name} is damaged")));
    }
    write_files(m, folder)
}

fn at_u16(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?) as usize)
}

fn at_u32(b: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?)).ok()
}

fn at_u64(b: &[u8], at: usize) -> Option<usize> {
    usize::try_from(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?)).ok()
}

/// Where a program's image ends in its file: past every byte its headers name (a 64-bit ELF's segments, sections and
/// section headers; a PE's headers, sections and COFF symbols, and the zeros after them up to its FileAlignment). What
/// lies after is carried data (a PE's overlay). None for a file that is neither, or whose headers can't be read.
pub fn image_end(file: &[u8]) -> Option<usize> {
    if file.starts_with(b"\x7fELF") {
        // (64-bit little-endian only: what the builder is)
        if file.get(4..6) != Some(&[2, 1]) {
            return None;
        }
        let (phoff, shoff) = (at_u64(file, 0x20)?, at_u64(file, 0x28)?);
        let (phsize, phnum) = (at_u16(file, 0x36)?, at_u16(file, 0x38)?);
        let (shsize, shnum) = (at_u16(file, 0x3A)?, at_u16(file, 0x3C)?);
        let mut end = 64usize.max(phoff.checked_add(phsize.checked_mul(phnum)?)?);
        if shnum > 0 {
            end = end.max(shoff.checked_add(shsize.checked_mul(shnum)?)?);
        }
        for i in 0..phnum {
            let p = phoff + i * phsize;
            end = end.max(at_u64(file, p + 8)?.checked_add(at_u64(file, p + 32)?)?);
        }
        for i in 0..shnum {
            let s = shoff + i * shsize;
            // (SHT_NOBITS: no bytes in the file)
            if at_u32(file, s + 4)? != 8 {
                end = end.max(at_u64(file, s + 0x18)?.checked_add(at_u64(file, s + 0x20)?)?);
            }
        }
        return Some(end);
    }
    if file.starts_with(b"MZ") {
        let pe = at_u32(file, 0x3C)?;
        if file.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
            return None;
        }
        let coff = pe + 4;
        let (sections, opt) = (at_u16(file, coff + 2)?, at_u16(file, coff + 16)?);
        let table = coff + 20 + opt;
        // (the section table, and SizeOfHeaders, in the optional header)
        let mut end = table.checked_add(sections.checked_mul(40)?)?.max(at_u32(file, coff + 20 + 60)?);
        for i in 0..sections {
            let s = table + i * 40;
            let (size, at) = (at_u32(file, s + 16)?, at_u32(file, s + 20)?);
            if size > 0 {
                end = end.max(at.checked_add(size)?);
            }
        }
        // (COFF symbols and their string table, which a linker may leave in a program)
        let (symbols, count) = (at_u32(file, coff + 8)?, at_u32(file, coff + 12)?);
        if symbols > 0 {
            let strings = symbols.checked_add(count.checked_mul(18)?)?;
            end = end.max(strings.checked_add(at_u32(file, strings)?)?);
        }
        // (the zeros a linker pads the file with after them, up to FileAlignment: llvm-mingw's lld does in a program
        // that keeps its COFF symbols, as a developer's build and the dist profile's do. Zeros only, and only bytes the
        // file has: anything else is past the image)
        let align = at_u32(file, coff + 20 + 36)?;
        if align.is_power_of_two() {
            let padded = end.checked_add(align - 1)? & !(align - 1);
            if file.get(end..padded).is_some_and(|pad| pad.iter().all(|&b| b == 0)) {
                end = padded;
            }
        }
        return Some(end);
    }
    None
}

/// The folder a carried tool was unpacked into by the last [`unpack`], by its member's name (`tools/<name>`).
pub fn tool_dir(name: &str) -> Option<PathBuf> {
    TOOLS.lock().unwrap_or_else(|p| p.into_inner()).get(name).cloned()
}

use build_core::key::to_hex as hex;

/// An unpacked pack.
#[derive(Clone, Debug)]
pub struct Unpacked {
    /// The tree's folder: the build's root.
    pub dir: PathBuf,
    /// The tree's hash (of its members').
    pub hash: String,
    /// The tree's files.
    pub files: u64,
    /// Each carried tool's folder.
    pub tools: BTreeMap<String, PathBuf>,
    /// Folders this call unpacked (the others were there).
    pub fresh: usize,
}

fn bad(why: impl Into<String>) -> Error {
    Error::player(
        "tools_incomplete",
        format!(
            "What this longshot-builder carries could not be unpacked ({}). Download longshot-builder again.",
            why.into()
        ),
    )
}

use build_core::fsx::{link_inside, relative_ok as path_ok, through_link};

fn read_exact(r: &mut impl Read, n: usize) -> Result<Vec<u8>> {
    let mut b = vec![0; n];
    r.read_exact(&mut b).map_err(|e| bad(e.to_string()))?;
    Ok(b)
}

fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes(b.try_into().unwrap_or_default())
}

/// One member of the pack.
struct Member<'a> {
    name: String,
    bytes: &'a [u8],
    sha: String,
}

/// The pack's members, from its table (their bytes are checked when they are unpacked: [`place`]).
fn members(pack: &[u8]) -> Result<Vec<Member<'_>>> {
    let short = || bad("the pack ends early");
    if pack.get(..8) != Some(&PACK_MAGIC[..]) {
        return Err(bad("not a pack of this format"));
    }
    let count = u32::from_le_bytes(pack.get(8..12).ok_or_else(short)?.try_into().unwrap_or_default()) as usize;
    let mut at = 12;
    // (a count no table of this size can hold is a damaged one: no room made for it)
    let mut out = Vec::with_capacity(count.min(pack.len() / 58));
    for _ in 0..count {
        let len = u16::from_le_bytes(pack.get(at..at + 2).ok_or_else(short)?.try_into().unwrap_or_default()) as usize;
        at += 2;
        let name = std::str::from_utf8(pack.get(at..at + len).ok_or_else(short)?)
            .map_err(|_| bad("a member's name is not UTF-8"))?
            .to_string();
        at += len;
        let head = pack.get(at..at + 48).ok_or_else(short)?;
        let (off, n) = (le_u64(&head[..8]) as usize, le_u64(&head[8..16]) as usize);
        let want = hex(&head[16..48]);
        at += 48;
        let bytes = pack.get(off..off.checked_add(n).ok_or_else(short)?).ok_or_else(short)?;
        let ok = name.strip_prefix("tree/").or_else(|| name.strip_prefix("tools/")).is_some_and(path_ok);
        if !ok {
            return Err(bad(format!("a member named {name:?}")));
        }
        out.push(Member { name, bytes, sha: want });
    }
    Ok(out)
}

/// `members` unpacked together into `dir` with `links` (a folder's files linked at a path in it), unless it is there
/// already, complete with `key`; whether it was unpacked now, and its files.
fn place(members: &[&Member<'_>], key: &str, dir: &Path, links: &[(PathBuf, PathBuf)]) -> Result<(bool, u64)> {
    let complete = |d: &Path| {
        let note = std::fs::read_to_string(d.join(COMPLETE)).unwrap_or_default();
        let mut w = note.split_whitespace();
        (w.next() == Some(key)).then(|| w.next().and_then(|n| n.parse().ok()).unwrap_or(0))
    };
    if let Some(n) = complete(dir) {
        return Ok((false, n));
    }
    for m in members {
        if hex(&Sha256::digest(m.bytes)) != m.sha {
            return Err(bad(format!("{} is damaged", m.name)));
        }
    }
    let parent = dir.parent().unwrap_or(dir);
    std::fs::create_dir_all(parent).at(parent)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let part = parent.join(format!("{name}.part-{}", std::process::id()));
    let _ = fsx::remove_tree(&part);
    let mut files = 0;
    for m in members {
        match write_files(m, &part) {
            Ok(n) => files += n,
            Err(e) => {
                let _ = fsx::remove_tree(&part);
                return Err(e);
            }
        }
    }
    for (from, rel) in links {
        if let Err(e) = link_dir(from, &part.join(rel)) {
            let _ = fsx::remove_tree(&part);
            return Err(e);
        }
    }
    fsx::write_atomic(&part.join(COMPLETE), format!("{key} {files}\n").as_bytes())?;
    if dir.exists() {
        // (another builder's that just won, or one a stopped unpack left half done: replaced)
        if complete(dir).is_some() {
            let _ = fsx::remove_tree(&part);
            return Ok((false, files));
        }
        fsx::remove_tree(dir)?;
    }
    if let Err(e) = fsx::rename(&part, dir) {
        let _ = fsx::remove_tree(&part);
        if complete(dir).is_none() {
            return Err(Error::io(dir, e));
        }
        return Ok((false, files));
    }
    Ok((true, files))
}

/// The tree's hash: of its members' names and recorded hashes (the pack's table alone).
fn tree_hash(all: &[Member<'_>]) -> String {
    let mut h = Sha256::new();
    for m in all {
        h.update(m.name.as_bytes());
        h.update(m.sha.as_bytes());
    }
    hex(&h.finalize())
}

/// Whether [`unpack`] of `pack` into `cache` would unpack its tree now (the first run of a release: seconds, minutes
/// on a hard drive), from the pack's table alone. A pack it cannot read: true (the unpack says why).
pub fn unpacks_now(pack: &[u8], cache: &Path) -> bool {
    let Ok(all) = members(pack) else { return true };
    let hash = tree_hash(&all);
    let note = std::fs::read_to_string(cache.join("sources").join(&hash[..16]).join(COMPLETE)).unwrap_or_default();
    note.split_whitespace().next() != Some(hash.as_str())
}

/// The release a pack carries (its tree's `VERSION`, in its `tree/files` member: a few small files, decoded alone) and
/// its tree's hash (16 digits); `None` when it carries none.
pub fn carried_release(pack: &[u8]) -> Option<(String, String)> {
    let all = members(pack).ok()?;
    let hash = tree_hash(&all)[..16].to_string();
    let files = all.iter().find(|m| m.name == "tree/files")?;
    if hex(&Sha256::digest(files.bytes)) != files.sha {
        return None;
    }
    let mut z = ruzstd::decoding::StreamingDecoder::new(files.bytes).ok()?;
    if read_exact(&mut z, MAGIC.len()).ok()? != MAGIC {
        return None;
    }
    loop {
        let len = u16::from_le_bytes(read_exact(&mut z, 2).ok()?.try_into().ok()?) as usize;
        if len == 0 {
            return None;
        }
        let rel = String::from_utf8(read_exact(&mut z, len).ok()?).ok()?;
        let _mode = read_exact(&mut z, 1).ok()?;
        let size = le_u64(&read_exact(&mut z, 8).ok()?);
        let _sha = read_exact(&mut z, 32).ok()?;
        // (a top-level file is small; anything larger here is not a pack of this format)
        let bytes = read_exact(&mut z, usize::try_from(size).ok().filter(|&n| n <= 16 << 20)?).ok()?;
        if rel == "VERSION" {
            return Some((String::from_utf8_lossy(&bytes).trim().to_string(), hash));
        }
    }
}

/// `pack` unpacked into the cache: its tree into `<cache>/sources/<hash>/` with each tool at `tools/<name>/` in it,
/// hard-linked from `<cache>/tools/<name>-<hash>/`, each folder unpacked only when it is not there already. Two
/// builders at once each unpack into a folder of their own and the first rename wins; a stopped one leaves a folder
/// with no [`COMPLETE`] note, never taken for a finished one.
pub fn unpack(pack: &[u8], cache: &Path) -> Result<Unpacked> {
    let all = members(pack)?;
    let tree: Vec<&Member<'_>> = all.iter().filter(|m| m.name.starts_with("tree/")).collect();
    if tree.is_empty() {
        return Err(bad("no tree in it"));
    }
    let tool_members: Vec<&Member<'_>> = all.iter().filter(|m| m.name.starts_with("tools/")).collect();
    let hash = tree_hash(&all);
    let dir = cache.join("sources").join(&hash[..16]);
    let mut fresh = 0;
    let mut cached = BTreeMap::new();
    let tools: BTreeMap<String, PathBuf> = tool_members
        .iter()
        .map(|m| {
            let name = m.name["tools/".len()..].to_string();
            (name.clone(), dir.join("tools").join(&name))
        })
        .collect();
    // (each tool once in the cache: the tree links its files)
    let complete_tree = std::fs::read_to_string(dir.join(COMPLETE))
        .ok()
        .is_some_and(|n| n.split_whitespace().next() == Some(hash.as_str()));
    if !complete_tree {
        // (the carried pack hashed whole, once: a download cut short or damaged is said as such)
        if !pack.is_empty() && std::ptr::eq(pack.as_ptr(), embedded().as_ptr()) {
            check_carried()?;
        }
        for m in &tool_members {
            let name = &m.name["tools/".len()..];
            let at = cache.join("tools").join(format!("{name}-{}", &m.sha[..16]));
            fresh += usize::from(place(&[*m], &m.sha, &at, &[])?.0);
            cached.insert(name.to_string(), at);
        }
    }
    let links: Vec<(PathBuf, PathBuf)> =
        cached.iter().map(|(name, at)| (at.clone(), PathBuf::from("tools").join(name))).collect();
    let (fresh_tree, files) = place(&tree, &hash, &dir, &links)?;
    fresh += usize::from(fresh_tree);
    *TOOLS.lock().unwrap_or_else(|p| p.into_inner()) = tools.clone();
    // (this release's folders marked used: the cache's budget keeps them by it, crate::storage)
    let mut current = vec![dir.clone()];
    for m in &tool_members {
        current.push(cache.join("tools").join(format!("{}-{}", &m.name["tools/".len()..], &m.sha[..16])));
    }
    for d in &current {
        let _ = std::fs::write(d.join(USED), b"");
    }
    Ok(Unpacked { dir, hash, files, tools, fresh })
}

/// The files of folder `from` placed at `to` (made when missing), each hard-linked where it can be, else copied, its
/// relative symbolic links kept as links (a carried tool into the unpacked tree; `from-source`'s tools into its source).
pub fn link_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).at(to)?;
    for e in std::fs::read_dir(from).at(from)? {
        let e = e.at(from)?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let kind = e.file_type().at(&src)?;
        if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&src).at(&src)?, &dst).at(&dst)?;
            #[cfg(not(unix))]
            return Err(bad(format!("a symbolic link, {}, on a system that unpacks none", src.display())));
        } else if kind.is_dir() {
            if e.file_name() != COMPLETE {
                link_dir(&src, &dst)?;
            }
        } else if e.file_name() != COMPLETE {
            fsx::link_or_copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// A member's files written under `dir`, each checked; how many.
fn write_files(m: &Member<'_>, dir: &Path) -> Result<u64> {
    let rel_err = |e: String| bad(format!("{}: {e}", m.name));
    let mut z = ruzstd::decoding::StreamingDecoder::new(m.bytes).map_err(|e| rel_err(e.to_string()))?;
    if read_exact(&mut z, MAGIC.len())? != MAGIC {
        return Err(rel_err("not a member of this format".into()));
    }
    let mut n = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let len = u16::from_le_bytes(read_exact(&mut z, 2)?.try_into().unwrap_or_default()) as usize;
        if len == 0 {
            let count = le_u64(&read_exact(&mut z, 8)?);
            if count != n {
                return Err(rel_err(format!("{n} files, it says {count}")));
            }
            return Ok(n);
        }
        let rel =
            String::from_utf8(read_exact(&mut z, len)?).map_err(|_| rel_err("a path that is not UTF-8".into()))?;
        if !path_ok(&rel) {
            return Err(rel_err(format!("a path outside its folder: {rel:?}")));
        }
        let mode = read_exact(&mut z, 1)?[0];
        let size = le_u64(&read_exact(&mut z, 8)?);
        let want = hex(&read_exact(&mut z, 32)?);
        let path = dir.join(&rel);
        if let Some(link) = through_link(dir, &rel) {
            return Err(rel_err(format!("{rel} goes through the link {link}")));
        }
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(rel_err(format!("{rel} is there twice")));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        if mode == 2 {
            let target = read_exact(&mut z, size as usize)?;
            if hex(&Sha256::digest(&target)) != want {
                return Err(rel_err(format!("{rel} is not the link it should be")));
            }
            let target = String::from_utf8(target).map_err(|_| rel_err(format!("{rel}: a link that is not UTF-8")))?;
            if !link_inside(&rel, &target) {
                return Err(rel_err(format!("{rel} links outside its folder ({target})")));
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &path).at(&path)?;
            #[cfg(not(unix))]
            return Err(rel_err(format!("{rel}: a symbolic link, on a system that unpacks none")));
            #[cfg(unix)]
            {
                n += 1;
                continue;
            }
        }
        let mut out = std::fs::File::create(&path).at(&path)?;
        let mut h = Sha256::new();
        let mut left = size;
        while left > 0 {
            let take = left.min(buf.len() as u64) as usize;
            z.read_exact(&mut buf[..take]).map_err(|e| rel_err(format!("{rel}: {e}")))?;
            h.update(&buf[..take]);
            out.write_all(&buf[..take]).at(&path)?;
            left -= take as u64;
        }
        if hex(&h.finalize()) != want {
            return Err(rel_err(format!("{rel} is not the file it should be")));
        }
        #[cfg(unix)]
        if mode == 1 {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).at(&path)?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        n += 1;
    }
}

// ---- the writer (`longshot-builder dev release sources`, `unpack`): the one that makes what the reader above reads ----

/// The tree a player's build reads, as tracked: the game's sources (`host/`, `game/`, `gpu/`, and the game's Rust, whose
/// key says whether the prebuilt `liblongshot_rs.a` added with `--add` is theirs: build-compile's `rust`), the
/// third-party code it compiles and the licences the game folder gets, the assets the finish makes its files from.
pub const TREE: [&str; 15] = [
    "Cargo.toml",
    "Cargo.lock",
    "longshot-rs",
    "host",
    "game",
    "gpu",
    "third_party",
    "assets/profiles",
    "assets/pipeline_states",
    "assets/prompt_faces",
    "assets/longshot.png",
    "assets/longshot.ico",
    "assets/addresses.ini",
    "LICENSE",
    "tools.lock",
];
/// What of [`TREE`] a player's build never reads: the third-party code only developers build (Tracy's client,
/// FidelityFX SDK's shader sources) and RenHDrum's Rust sources (its built library comes as a file added: `--add`).
pub const LEFT_OUT: [&str; 7] = [
    "third_party/a64-kit/",
    "third_party/tracy/",
    "third_party/fidelityfx-sdk/",
    "third_party/renhdrum/src/",
    "third_party/renhdrum/vendor/",
    "third_party/renhdrum/Cargo.",
    "third_party/renhdrum/build.rs",
];
/// The zstd program's arguments for a member: one thread and its checksum, so the same files give the same bytes.
pub const MEMBER_ZSTD: [&str; 5] = ["-19", "-T1", "-q", "-c", "--check"];

/// A member's file: its bytes from a file or given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// The file at this path (read when the member is made).
    File(PathBuf),
    /// These bytes (a link's: its target).
    Bytes(Vec<u8>),
}

/// One file of a member: its path in the member, its mode (0 plain, 1 executable, 2 a link) and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Relative, `/` separated.
    pub path: String,
    /// 0 plain, 1 executable, 2 a relative symbolic link (its bytes the target).
    pub mode: u8,
    /// Its bytes.
    pub body: Body,
}

/// The files of the checkout at `root` tracked under `paths`, relative with `/`, sorted: jj's in a jj checkout (its
/// working copy as the last jj command saw it: `jj status` first takes in what changed since), else git's.
pub fn tracked(root: &Path, paths: &[&str]) -> std::result::Result<Vec<String>, String> {
    if root.join(".jj").is_dir() {
        let out = build_core::child::output(
            build_core::child::command("jj")
                .args(["file", "list", "--ignore-working-copy", "-r", "@"])
                .args(paths)
                .current_dir(root),
        );
        if let Some(out) = out.ok().filter(|o| o.status.success()) {
            let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().replace('\\', "/"))
                .filter(|l| !l.is_empty())
                .collect();
            files.sort();
            return Ok(files);
        }
    }
    let out = build_core::child::output(
        build_core::child::command("git").args(["ls-files", "-z", "--"]).args(paths).current_dir(root),
    )
    .map_err(|e| format!("git did not start: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let mut files: Vec<String> =
        String::from_utf8_lossy(&out.stdout).split('\0').filter(|f| !f.is_empty()).map(str::to_string).collect();
    files.sort();
    Ok(files)
}

/// Every file of the folder at `root` under `paths` (each a folder or a file), relative with `/`, sorted: [`tracked`]'s
/// list for a folder that is no checkout, a commit's archive unpacked (`from-source`), whose files are all tracked.
pub fn listed(root: &Path, paths: &[&str]) -> std::result::Result<Vec<String>, String> {
    let mut files = Vec::new();
    for p in paths {
        let full = root.join(p);
        if full.is_file() {
            files.push(p.to_string());
        } else if full.is_dir() {
            for rel in fsx::files_under(&full).map_err(|e| e.to_string())? {
                files.push(format!("{p}/{}", rel.to_string_lossy().replace('\\', "/")));
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Every file under `folder` (links to files followed, a link to a folder not walked): (relative path with `/`, path).
fn files_under(folder: &Path) -> std::result::Result<Vec<(String, PathBuf)>, String> {
    Ok(crate::tools::archive::entries(folder)?.into_iter().filter(|(_, p)| p.is_file()).collect())
}

fn executable_file(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| crate::tools::archive::executable(&m))
}

/// The tree's files, sorted: `tracked` (the checkout's tracked files under [`TREE`], at `root`) but [`LEFT_OUT`],
/// `VERSION` (the release's name), the third-party objects at `third_party_objects/` (`longshot-builder compile
/// --third-party-objects`'s folder, its `set.json` in it) and each of `add` (`<path in the tree>=<file>`, over a tracked
/// file of that path). A tree with no pipeline state list is refused: a release ships them.
pub fn tree_entries(
    root: &Path,
    tracked: &[String],
    version: &str,
    objects: Option<&Path>,
    add: &[String],
) -> std::result::Result<Vec<Entry>, String> {
    tree_entries_leaving(root, tracked, version, objects, add, &LEFT_OUT)
}

/// [`tree_entries`] with another list of what is left out than [`LEFT_OUT`] (`from-source`: the source's own).
pub fn tree_entries_leaving(
    root: &Path,
    tracked: &[String],
    version: &str,
    objects: Option<&Path>,
    add: &[String],
    left_out: &[&str],
) -> std::result::Result<Vec<Entry>, String> {
    let mut all: BTreeMap<String, Entry> = BTreeMap::new();
    let mut put = |path: String, mode: u8, body: Body| {
        all.insert(path.clone(), Entry { path, mode, body });
    };
    for rel in tracked {
        let full = root.join(rel);
        if left_out.iter().any(|l| rel.starts_with(l)) || !full.is_file() {
            continue;
        }
        put(rel.clone(), u8::from(executable_file(&full)), Body::File(full));
    }
    put("VERSION".into(), 0, Body::Bytes(format!("{version}\n").into_bytes()));
    if let Some(objects) = objects {
        if !objects.join("set.json").is_file() {
            return Err(format!(
                "{} has no set.json (longshot-builder compile --third-party-objects makes it)",
                objects.display()
            ));
        }
        for (rel, full) in files_under(objects)? {
            put(format!("third_party_objects/{rel}"), 0, Body::File(full));
        }
    }
    for spec in add {
        let (rel, file) = spec.split_once('=').unwrap_or((spec, ""));
        let file = Path::new(file);
        if rel.is_empty() || !file.is_file() {
            return Err(format!("--add {spec}: not <path in the tree>=<file>"));
        }
        put(rel.replace('\\', "/"), u8::from(executable_file(file)), Body::File(file.to_path_buf()));
    }
    if !all.keys().any(|r| r.starts_with("assets/pipeline_states/") && r.ends_with(".bin.xz")) {
        return Err("assets/pipeline_states/ holds no pipeline state list: a release ships them".into());
    }
    Ok(all.into_values().collect())
}

/// A tool's folder as a member's files, sorted: its files, and its relative links kept as links (mode 2); a link
/// leading out of the folder is refused.
pub fn tool_entries(folder: &Path) -> std::result::Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (rel, full) in crate::tools::archive::entries(folder)? {
        let meta = std::fs::symlink_metadata(&full).map_err(|e| format!("{}: {e}", full.display()))?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&full).map_err(|e| format!("{}: {e}", full.display()))?;
            let target = target.to_string_lossy().into_owned();
            if !link_inside(&rel, &target) {
                return Err(format!("{}: {rel} links outside the tool ({target})", folder.display()));
            }
            out.push(Entry { path: rel, mode: 2, body: Body::Bytes(target.into_bytes()) });
        } else if meta.is_file() {
            out.push(Entry {
                path: rel,
                mode: u8::from(crate::tools::archive::executable(&meta)),
                body: Body::File(full),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// The tree member a file goes in: `tree/<its top folder>` (`third_party/<name>`, `assets/<name>` and `gpu/<name>` each
/// apart: a change compresses again only its own part), `tree/files` for the top-level files.
pub fn group(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.as_slice() {
        [_] => "tree/files".into(),
        [top @ ("third_party" | "assets" | "gpu"), name, _, ..] => format!("tree/{top}-{name}"),
        [top, ..] => format!("tree/{top}"),
        [] => "tree/files".into(),
    }
}

/// A member's bytes before compression: [`MAGIC`], its files, the end (the format above).
pub fn member_bytes(entries: &[Entry]) -> std::result::Result<Vec<u8>, String> {
    let mut out = MAGIC.to_vec();
    for e in entries {
        let read;
        let data: &[u8] = match &e.body {
            Body::Bytes(b) => b,
            Body::File(p) => {
                read = std::fs::read(p).map_err(|err| format!("{}: {err}", p.display()))?;
                &read
            }
        };
        let name = e.path.as_bytes();
        let len = u16::try_from(name.len()).map_err(|_| format!("{}: a path too long", e.path))?;
        out.extend(len.to_le_bytes());
        out.extend(name);
        out.push(e.mode);
        out.extend((data.len() as u64).to_le_bytes());
        out.extend(Sha256::digest(data));
        out.extend(data);
    }
    out.extend(0u16.to_le_bytes());
    out.extend((entries.len() as u64).to_le_bytes());
    Ok(out)
}

/// `(name, compressed bytes)` as the pack: its table, then the members' bytes in that order.
pub fn pack_bytes(members: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut head = PACK_MAGIC.to_vec();
    head.extend((members.len() as u32).to_le_bytes());
    let mut off = head.len() + members.iter().map(|(n, _)| 2 + n.len() + 48).sum::<usize>();
    for (name, data) in members {
        head.extend((name.len() as u16).to_le_bytes());
        head.extend(name.as_bytes());
        head.extend((off as u64).to_le_bytes());
        head.extend((data.len() as u64).to_le_bytes());
        head.extend(Sha256::digest(data));
        off += data.len();
    }
    for (_, data) in members {
        head.extend(data);
    }
    head
}

/// `raw` as one zstd frame, by the zstd program at [`MEMBER_ZSTD`] (the same bytes each time: one thread).
pub fn compress(raw: &[u8]) -> std::result::Result<Vec<u8>, String> {
    use std::process::Stdio;
    let mut child = build_core::child::spawn(
        build_core::child::command(crate::tools::archive::zstd()?)
            .args(MEMBER_ZSTD)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|e| format!("zstd did not start: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("zstd's input")?;
    let (fed, out) = std::thread::scope(|s| {
        let feed = s.spawn(move || stdin.write_all(raw));
        let out = child.wait_with_output();
        (feed.join(), out)
    });
    let out = out.map_err(|e| format!("zstd: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("zstd failed: {}", &err[err.len().saturating_sub(300)..]));
    }
    fed.map_err(|_| "zstd's input".to_string())?.map_err(|e| format!("zstd's input: {e}"))?;
    Ok(out.stdout)
}

/// `raw` as one zstd frame, quickly: the zstd program at `-3` when this PC has one, else ruzstd's fastest level (no
/// program needed). A pack for this PC alone (`from-source`'s), never a release's: its bytes are not [`compress`]'s.
pub fn compress_fast(raw: &[u8]) -> std::result::Result<Vec<u8>, String> {
    use std::process::Stdio;
    let Ok(zstd) = crate::tools::archive::zstd() else {
        return Ok(ruzstd::encoding::compress_to_vec(raw, ruzstd::encoding::CompressionLevel::Fastest));
    };
    let mut child = build_core::child::spawn(
        build_core::child::command(zstd)
            .args(["-3", "-T0", "-q", "-c", "--check"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )
    .map_err(|e| format!("zstd did not start: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("zstd's input")?;
    let (fed, out) = std::thread::scope(|s| {
        let feed = s.spawn(move || stdin.write_all(raw));
        let out = child.wait_with_output();
        (feed.join(), out)
    });
    let out = out.map_err(|e| format!("zstd: {e}"))?;
    if !out.status.success() {
        return Err("zstd failed".into());
    }
    fed.map_err(|_| "zstd's input".to_string())?.map_err(|e| format!("zstd's input: {e}"))?;
    Ok(out.stdout)
}

/// A member's compressor: its raw bytes to one zstd frame ([`compress`] for a release). Called from several threads at
/// once, one member each.
pub type Compressor<'a> = &'a (dyn Fn(&[u8]) -> std::result::Result<Vec<u8>, String> + Sync);

/// The magic of the zstd skippable frame a release's member ends with (zstd's 0x184D2A50 to 0x184D2A5F: every zstd
/// reader passes over one, and this format's reader never reads past a member's last file). Its 32 bytes are the
/// member's [`member_key`].
pub const KEY_FRAME_MAGIC: u32 = 0x184D_2A5E;
/// That frame's length: its magic, its size (32, LE), the key.
pub const KEY_FRAME_LEN: usize = 40;

/// A member's reuse key: the SHA-256 of what compresses it (`settings`: the zstd program's version and [`MEMBER_ZSTD`],
/// [`release_settings`]) and of its raw bytes' SHA-256. Two members with the same key are the same bytes compressed the
/// same way: the same frame.
pub fn member_key(raw: &[u8], settings: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"longshot pack member key 1\0");
    h.update(settings.as_bytes());
    h.update(b"\0");
    h.update(Sha256::digest(raw));
    h.finalize().into()
}

/// The skippable frame a release's member ends with: [`KEY_FRAME_MAGIC`], 32, the key.
pub fn key_frame(key: &[u8; 32]) -> [u8; KEY_FRAME_LEN] {
    let mut f = [0u8; KEY_FRAME_LEN];
    f[..4].copy_from_slice(&KEY_FRAME_MAGIC.to_le_bytes());
    f[4..8].copy_from_slice(&32u32.to_le_bytes());
    f[8..].copy_from_slice(key);
    f
}

/// A member's bytes split into its zstd frame and the key its last frame names; None when it ends with no key frame (a
/// pack from before them, or one made for this PC alone).
pub fn split_key(member: &[u8]) -> Option<(&[u8], [u8; 32])> {
    let at = member.len().checked_sub(KEY_FRAME_LEN)?;
    let tail = &member[at..];
    if tail[..4] != KEY_FRAME_MAGIC.to_le_bytes() || tail[4..8] != 32u32.to_le_bytes() {
        return None;
    }
    Some((&member[..at], tail[8..].try_into().ok()?))
}

/// What compresses a release's members, for [`member_key`]: the zstd program's version (its `-V`) and [`MEMBER_ZSTD`].
pub fn release_settings() -> std::result::Result<String, String> {
    let out = build_core::child::command(crate::tools::archive::zstd()?)
        .arg("-V")
        .output()
        .map_err(|e| format!("zstd -V: {e}"))?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || version.is_empty() {
        return Err("zstd -V said no version".into());
    }
    Ok(format!("{version}; {}", MEMBER_ZSTD.join(" ")))
}

/// How [`write_with`] makes the members.
pub struct Packing<'a> {
    /// Each member's compressor.
    pub zstd: Compressor<'a>,
    /// What `zstd` is ([`release_settings`] for [`compress`]): each member then ends with its key's frame
    /// ([`key_frame`]), and the member of `reuse` of the same name and key is taken instead of compressing it again.
    /// None: no key frames, nothing reused (a pack for this PC alone).
    pub settings: Option<String>,
    /// The members of an earlier pack (`(name, bytes)`, [`reusable`]: a signed release's, `dev release last-objects`).
    pub reuse: &'a [(String, Vec<u8>)],
    /// How many members are made at once (each by one zstd of one thread); at least 1.
    pub jobs: usize,
}

/// What [`write`] made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    /// Its members.
    pub members: usize,
    /// The files in them.
    pub files: usize,
    /// The members' bytes before compression.
    pub raw: u64,
    /// The pack's bytes.
    pub size: u64,
    /// The pack's SHA-256.
    pub sha256: String,
    /// The members taken from the earlier pack, not compressed again.
    pub reused: usize,
    /// Their bytes before compression.
    pub reused_raw: u64,
}

/// The members of a pack, each checked by its table's SHA-256 (one that fails is left out): what [`Packing::reuse`]
/// takes.
pub fn reusable(pack: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    Ok(members(pack)?
        .into_iter()
        .filter(|m| hex(&Sha256::digest(m.bytes)) == m.sha)
        .map(|m| (m.name, m.bytes.to_vec()))
        .collect())
}

/// `old`'s bytes when they are `raw` compressed as `key` says: its key frame's key `key`, and its frame decompressed
/// `raw`, compared whole (so a key that lied is never taken).
fn reuse_of(old: &[u8], raw: &[u8], key: &[u8; 32]) -> Option<Vec<u8>> {
    let (frame, k) = split_key(old)?;
    if &k != key {
        return None;
    }
    let mut z = ruzstd::decoding::StreamingDecoder::new(frame).ok()?;
    let mut back = Vec::with_capacity(raw.len());
    z.read_to_end(&mut back).ok()?;
    (back == raw).then(|| old.to_vec())
}

/// The pack of `tree` (each [`group`] a member, by name) and `tools` (`(name, folder)`, each a member `tools/<name>`,
/// in that order), compressed by `zstd`, written to `out` through `out.new`: [`write_with`] with no key frames and
/// nothing reused, as many members at once as this PC has threads.
pub fn write(
    out: &Path,
    tree: &[Entry],
    tools: &[(String, PathBuf)],
    zstd: Compressor<'_>,
) -> std::result::Result<Written, String> {
    let jobs = std::thread::available_parallelism().map_or(1, usize::from);
    write_with(out, tree, tools, &Packing { zstd, settings: None, reuse: &[], jobs })
}

/// [`write`] as `p` says: the members made by a pool of `p.jobs` threads (each its raw bytes, then the member of
/// `p.reuse` with its name and key, else compressed), put in the pack in their order whichever ends first, so the bytes
/// are those of one member after another.
pub fn write_with(
    out: &Path,
    tree: &[Entry],
    tools: &[(String, PathBuf)],
    p: &Packing<'_>,
) -> std::result::Result<Written, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // (a member made: its bytes, its raw size, whether they were reused)
    type Made = std::result::Result<(Vec<u8>, u64, bool), String>;
    let mut groups: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for e in tree {
        groups.entry(group(&e.path)).or_default().push(e.clone());
    }
    let mut todo: Vec<(String, Vec<Entry>)> = groups.into_iter().collect();
    for (name, folder) in tools {
        todo.push((format!("tools/{name}"), tool_entries(folder)?));
    }
    let files = todo.iter().map(|(_, e)| e.len()).sum::<usize>();
    let old: BTreeMap<&str, &[u8]> = p.reuse.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let make = |(name, entries): &(String, Vec<Entry>)| -> Made {
        let raw = member_bytes(entries)?;
        let Some(settings) = &p.settings else {
            return Ok(((p.zstd)(&raw)?, raw.len() as u64, false));
        };
        let key = member_key(&raw, settings);
        if let Some(b) = old.get(name.as_str()).and_then(|o| reuse_of(o, &raw, &key)) {
            return Ok((b, raw.len() as u64, true));
        }
        let mut b = (p.zstd)(&raw)?;
        b.extend(key_frame(&key));
        Ok((b, raw.len() as u64, false))
    };
    let slots: Vec<Mutex<Option<Made>>> = todo.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..p.jobs.clamp(1, todo.len().max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = todo.get(i) else { break };
                    let made = make(job);
                    let failed = made.is_err();
                    *slots[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(made);
                    if failed {
                        // (the first failure ends the pool: no member after it is started)
                        next.store(todo.len(), Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let (mut members, mut raw_size, mut reused, mut reused_raw) = (Vec::new(), 0u64, 0usize, 0u64);
    for ((name, _), slot) in todo.iter().zip(slots) {
        let made = slot.into_inner().unwrap_or_else(|e| e.into_inner());
        let (bytes, raw, again) = made.ok_or_else(|| format!("{name}: not made (a member before it failed)"))??;
        raw_size += raw;
        if again {
            reused += 1;
            reused_raw += raw;
        }
        members.push((name.clone(), bytes));
    }
    let data = pack_bytes(&members);
    if let Some(parent) = std::path::absolute(out).ok().as_deref().and_then(Path::parent) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut new = out.as_os_str().to_owned();
    new.push(".new");
    std::fs::write(&new, &data).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::rename(&new, out).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(Written {
        members: members.len(),
        files,
        raw: raw_size,
        size: data.len() as u64,
        sha256: hex(&Sha256::digest(&data)),
        reused,
        reused_raw,
    })
}

/// A pack's tree written into `folder` and each tool into `folder/tools/<name>`, every member's and file's SHA-256
/// checked (as a release's program unpacks it, into a plain folder: a release's check of what it carries); the tree's
/// files.
pub fn unpack_into(pack: &[u8], folder: &Path) -> Result<u64> {
    let mut n = 0;
    for m in members(pack)? {
        if hex(&Sha256::digest(m.bytes)) != m.sha {
            return Err(bad(format!("{} is damaged", m.name)));
        }
        match m.name.strip_prefix("tools/") {
            Some(tool) => {
                write_files(&m, &folder.join("tools").join(tool))?;
            }
            None => n += write_files(&m, folder)?,
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A member of `files`, as [`write`] makes one (ruzstd's fastest level for the zstd program's: the reader takes any).
    fn member(files: &[(&str, &[u8])]) -> Vec<u8> {
        let modes: Vec<(&str, u8, &[u8])> = files.iter().map(|(p, d)| (*p, 0, *d)).collect();
        member_modes(&modes)
    }

    /// A member of `(path, mode, bytes)` entries (mode 2: a link, its bytes the target).
    fn member_modes(files: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let entries: Vec<Entry> = files
            .iter()
            .map(|(p, mode, d)| Entry { path: p.to_string(), mode: *mode, body: Body::Bytes(d.to_vec()) })
            .collect();
        fast_zstd(&member_bytes(&entries).unwrap()).unwrap()
    }

    fn fast_zstd(raw: &[u8]) -> std::result::Result<Vec<u8>, String> {
        Ok(ruzstd::encoding::compress_to_vec(raw, ruzstd::encoding::CompressionLevel::Fastest))
    }

    fn pack(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
        pack_bytes(&members.iter().map(|(n, b)| (n.to_string(), b.clone())).collect::<Vec<_>>())
    }

    /// One unpack at a time among the tests: each sets the process's tool table ([`tool_dir`]).
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|p| p.into_inner())
    }

    use crate::scratch;

    #[test]
    fn unpacked_once_and_tools_kept_across_releases() {
        let _one = serial();
        let cache = scratch("once");
        let tool = member(&[("bin/clang", b"#!clang")]);
        let one = pack(&[
            ("tree/host", member(&[("host/a.c", b"int a;")])),
            ("tree/assets", member(&[("assets/x/y.bin", b"\x00\x01")])),
            ("tools/clang", tool.clone()),
        ]);
        let u = unpack(&one, &cache).unwrap();
        assert_eq!((u.files, u.fresh), (2, 2));
        assert_eq!(std::fs::read(u.dir.join("host/a.c")).unwrap(), b"int a;");
        assert_eq!(std::fs::read(u.tools["clang"].join("bin/clang")).unwrap(), b"#!clang");
        assert_eq!(u.tools["clang"], u.dir.join("tools/clang"));
        assert_eq!(tool_dir("clang"), Some(u.tools["clang"].clone()));
        assert_eq!(unpack(&one, &cache).unwrap().fresh, 0, "all there: nothing unpacked");
        // (the next release: one source changed, the same tool, which is not unpacked again)
        let two = pack(&[("tree/host", member(&[("host/a.c", b"int b;")])), ("tools/clang", tool)]);
        let v = unpack(&two, &cache).unwrap();
        assert_ne!(v.dir, u.dir);
        assert_eq!(v.fresh, 1, "the tree alone");
        assert_eq!(std::fs::read(v.tools["clang"].join("bin/clang")).unwrap(), b"#!clang");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let ino = |p: &Path| std::fs::metadata(p).unwrap().ino();
            assert_eq!(ino(&v.tools["clang"].join("bin/clang")), ino(&u.tools["clang"].join("bin/clang")), "linked");
        }
        // (a folder another builder left half done is replaced)
        std::fs::remove_file(v.dir.join(COMPLETE)).unwrap();
        assert_eq!(unpack(&two, &cache).unwrap().fresh, 1);
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn a_pack_unpacked_before_is_not_hashed_again() {
        let _one = serial();
        let cache = scratch("unhashed");
        let mut one = pack(&[("tree/host", member(&[("host/a.c", b"int a;")]))]);
        assert_eq!(unpack(&one, &cache).unwrap().fresh, 1);
        // (a member's bytes changed after the table: only an unpack reads them)
        let last = one.len() - 3;
        one[last] ^= 0xff;
        assert_eq!(unpack(&one, &cache).unwrap().fresh, 0, "the folder there: the pack's table alone read");
        let _ = std::fs::remove_dir_all(&cache);
        assert!(unpack(&one, &cache).is_err(), "unpacked anew: its bytes checked");
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn each_unpack_marks_its_folders_used() {
        let _one = serial();
        let cache = scratch("used");
        let p = pack(&[("tree/host", member(&[("host/a.c", b"1")])), ("tools/clang", member(&[("bin/c", b"1")]))]);
        let u = unpack(&p, &cache).unwrap();
        let tool = std::fs::read_dir(cache.join("tools")).unwrap().flatten().next().unwrap().path();
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
        for d in [&u.dir, &tool] {
            std::fs::File::options().write(true).open(d.join(USED)).unwrap().set_modified(when).unwrap();
        }
        unpack(&p, &cache).unwrap();
        for d in [&u.dir, &tool] {
            let age = std::fs::metadata(d.join(USED)).unwrap().modified().unwrap().elapsed().unwrap_or_default();
            assert!(age < std::time::Duration::from_secs(3600), "{}", d.display());
        }
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn the_carried_release_is_read_from_the_pack_and_an_unpack_known_before_it_runs() {
        let _one = serial();
        let cache = scratch("release");
        let p = pack(&[
            ("tree/files", member(&[("VERSION", b"nightly-7\n"), ("README", b"r")])),
            ("tree/host", member(&[("host/a.c", b"int a;")])),
        ]);
        let (release, hash) = carried_release(&p).unwrap();
        assert_eq!((release.as_str(), hash.len()), ("nightly-7", 16));
        assert!(carried_release(&pack(&[("tree/host", member(&[("host/a.c", b"x")]))])).is_none());
        assert!(unpacks_now(&p, &cache));
        let u = unpack(&p, &cache).unwrap();
        assert!(!unpacks_now(&p, &cache) && u.hash.starts_with(&hash));
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn links_stay_inside() {
        assert!(link_inside("lib", "usr/lib"));
        assert!(link_inside("a/b/c", "../../x"));
        assert!(!link_inside("a/b", "../../x"));
        assert!(!link_inside("a", "/etc/passwd"));
        assert!(!link_inside("a", ""));
    }

    #[cfg(unix)]
    #[test]
    fn nested_links_do_not_escape() {
        let _one = serial();
        let cache = scratch("nested");
        // (each link alone is inside: a/b -> .. is a/, a/b/c -> .. would be a/b/.. = a/ too, but written through the
        // link a/b it lands at the folder's parent's parent)
        let hostile = member_modes(&[("a/b", 2, b".."), ("a/b/c", 2, b".."), ("a/b/c/escaped", 0, b"x")]);
        assert!(unpack(&pack(&[("tree/x", hostile)]), &cache).is_err());
        assert!(!cache.join("escaped").exists() && !cache.parent().unwrap().join("escaped").exists());
        // (nor a file written over a link a member made)
        let over = member_modes(&[("a/l", 2, b"x"), ("a/l", 0, b"y")]);
        assert!(unpack(&pack(&[("tree/x", over)]), &cache).is_err());
        // (a link inside, then files beside it: fine)
        let fine = member_modes(&[("usr/lib/a.so", 0, b"a"), ("lib", 2, b"usr/lib")]);
        assert!(unpack(&pack(&[("tree/x", fine)]), &cache).is_ok());
        let _ = std::fs::remove_dir_all(&cache);
    }

    #[test]
    fn a_damaged_or_hostile_pack_is_refused() {
        let _one = serial();
        let cache = scratch("bad");
        assert!(unpack(&pack(&[("tree/x", member(&[("../escape", b"x")]))]), &cache).is_err());
        assert!(!cache.join("escape").exists());
        assert!(unpack(&pack(&[("tools/../x", member(&[("a", b"x")]))]), &cache).is_err());
        let mut p = pack(&[("tree/host", member(&[("host/a.c", b"int a;")]))]);
        let last = p.len() - 3;
        p[last] ^= 0xff;
        assert!(unpack(&p, &cache).is_err(), "a member whose SHA-256 is not its own");
        assert!(unpack(b"not a pack", &cache).is_err());
        assert!(unpack(&pack(&[("tools/clang", member(&[("a", b"x")]))]), &cache).is_err(), "no tree");
        let leftovers: Vec<std::fs::DirEntry> =
            std::fs::read_dir(cache.join("sources")).map(|d| d.flatten().collect()).unwrap_or_default();
        assert!(leftovers.is_empty(), "nothing half unpacked is left: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&cache);
    }

    /// A small checkout: two tracked sources, one left out, a pipeline state list when `list`.
    fn checkout(d: &Path, list: bool) -> Vec<String> {
        let mut files = vec!["LICENSE", "host/a.c", "third_party/tracy/t.c", "gpu/vk/x/y.cpp"];
        if list {
            files.push("assets/pipeline_states/contrib/someone.bin.xz");
        }
        for f in &files {
            std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
            std::fs::write(d.join(f), f.as_bytes()).unwrap();
        }
        files.iter().map(|f| f.to_string()).collect()
    }

    #[test]
    fn the_tree_carries_the_pipeline_state_lists_and_its_version() {
        let d = scratch("tree-entries");
        let tracked = checkout(&d, false);
        let e = tree_entries(&d, &tracked, "v0.0.0-test", None, &[]).unwrap_err();
        assert!(e.contains("no pipeline state list"), "{e}");
        let tracked = checkout(&d, true);
        let names: Vec<String> =
            tree_entries(&d, &tracked, "v0.0.0-test", None, &[]).unwrap().into_iter().map(|e| e.path).collect();
        assert!(names.contains(&"assets/pipeline_states/contrib/someone.bin.xz".to_string()));
        assert!(names.contains(&"VERSION".to_string()) && names.contains(&"host/a.c".to_string()));
        assert!(!names.iter().any(|n| n.starts_with("third_party/tracy/")), "left out");
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        assert!(tree_entries(&d, &tracked, "v", Some(&d.join("no-objects")), &[]).is_err(), "no set.json");
        assert!(tree_entries(&d, &tracked, "v", None, &["host/b.c".into()]).is_err(), "not <path>=<file>");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_members_are_grouped_by_top_folder() {
        assert_eq!(group("VERSION"), "tree/files");
        assert_eq!(group("host/a.c"), "tree/host");
        assert_eq!(group("third_party/imgui/a.h"), "tree/third_party-imgui");
        assert_eq!(group("third_party/LICENSE"), "tree/third_party");
        assert_eq!(group("gpu/vk/x.cpp"), "tree/gpu-vk");
        assert_eq!(group("assets/longshot.png"), "tree/assets");
        assert_eq!(group("third_party_objects/set.json"), "tree/third_party_objects");
    }

    #[cfg(unix)]
    #[test]
    fn what_the_writer_writes_the_reader_reads_the_same_each_time() {
        use std::os::unix::fs::PermissionsExt;
        let d = scratch("writer");
        let tracked = checkout(&d, true);
        std::fs::create_dir_all(d.join("objects/linux")).unwrap();
        std::fs::write(d.join("objects/set.json"), b"{}").unwrap();
        std::fs::write(d.join("objects/linux/a.o"), b"obj").unwrap();
        std::fs::write(d.join("renhdrum.so"), b"lib").unwrap();
        let tool = d.join("bundle/clang");
        std::fs::create_dir_all(tool.join("usr/lib")).unwrap();
        std::fs::write(tool.join("usr/lib/a.so"), b"a").unwrap();
        std::fs::write(tool.join("clang"), b"#!clang").unwrap();
        std::fs::set_permissions(tool.join("clang"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("usr/lib", tool.join("lib")).unwrap();
        let add = ["third_party/renhdrum/renhdrum.so=".to_string() + &d.join("renhdrum.so").display().to_string()];
        let tree = tree_entries(&d, &tracked, "nightly-7", Some(&d.join("objects")), &add).unwrap();
        let tools = [("clang".to_string(), tool.clone())];
        let out = d.join("out/sources.pack");
        let w = write(&out, &tree, &tools, &fast_zstd).unwrap();
        let first = std::fs::read(&out).unwrap();
        assert_eq!((w.size, w.files), (first.len() as u64, tree.len() + 3));
        assert_eq!(write(&out, &tree, &tools, &fast_zstd).unwrap(), w, "the same files, the same bytes");
        assert_eq!(std::fs::read(&out).unwrap(), first);
        assert_eq!(carried_release(&first).unwrap().0, "nightly-7");
        let into = d.join("carried");
        assert_eq!(unpack_into(&first, &into).unwrap(), tree.len() as u64);
        assert_eq!(std::fs::read(into.join("host/a.c")).unwrap(), b"host/a.c");
        assert_eq!(std::fs::read(into.join("third_party_objects/linux/a.o")).unwrap(), b"obj");
        assert_eq!(std::fs::read(into.join("third_party/renhdrum/renhdrum.so")).unwrap(), b"lib");
        assert_eq!(std::fs::read(into.join("VERSION")).unwrap(), b"nightly-7\n");
        let clang = into.join("tools/clang");
        assert_eq!(std::fs::metadata(clang.join("clang")).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_link(clang.join("lib")).unwrap(), Path::new("usr/lib"));
        assert!(unpack_into(&first, &into).is_err(), "nothing written over what is there");
        // (and the cache's unpack, a player's, reads the same pack)
        let _one = serial();
        let u = unpack(&first, &d.join("cache")).unwrap();
        assert_eq!(u.files, tree.len() as u64);
        assert_eq!(std::fs::read(u.tools["clang"].join("clang")).unwrap(), b"#!clang");
        // (a tool whose link leads out of it is refused)
        std::os::unix::fs::symlink("../../etc", tool.join("out")).unwrap();
        assert!(tool_entries(&tool).unwrap_err().contains("links outside the tool"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn entries(files: &[(&str, &[u8])]) -> Vec<Entry> {
        files.iter().map(|(p, d)| Entry { path: p.to_string(), mode: 0, body: Body::Bytes(d.to_vec()) }).collect()
    }

    /// The pack of `tree` as a release's (key frames, `settings`), reusing `reuse`'s members, `jobs` at once: its bytes,
    /// what was written, and how many members were compressed.
    fn release_pack(
        d: &Path,
        tree: &[Entry],
        settings: &str,
        reuse: &[(String, Vec<u8>)],
        jobs: usize,
    ) -> (Vec<u8>, Written, usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let zstd = |raw: &[u8]| {
            calls.fetch_add(1, Ordering::Relaxed);
            fast_zstd(raw)
        };
        let out = d.join("sources.pack");
        let p = Packing { zstd: &zstd, settings: Some(settings.into()), reuse, jobs };
        let w = write_with(&out, tree, &[], &p).unwrap();
        (std::fs::read(&out).unwrap(), w, calls.into_inner())
    }

    fn a_tree(gpu: &[u8]) -> Vec<Entry> {
        entries(&[
            ("VERSION", b"nightly-2\n"),
            ("host/a.c", &[1; 5000]),
            ("gpu/vk/a.cpp", gpu),
            ("third_party/sdl/a.c", &[3; 7000]),
            ("third_party/zlib/z.c", &[4; 900]),
        ])
    }

    #[test]
    fn a_reused_member_is_the_bytes_compressing_it_again_gives() {
        let d = scratch("reuse-same");
        let tree = a_tree(b"gpu");
        let (fresh, w, calls) = release_pack(&d, &tree, "zstd 1; -19", &[], 2);
        assert_eq!((w.members, calls, w.reused), (5, 5, 0));
        let old = reusable(&fresh).unwrap();
        assert!(old.iter().all(|(_, b)| split_key(b).is_some()), "every member ends with its key");
        let (again, w2, calls) = release_pack(&d, &tree, "zstd 1; -19", &old, 2);
        assert_eq!(again, fresh, "the same bytes as compressing them again");
        assert_eq!((calls, w2.reused, w2.reused_raw), (0, 5, w.raw));
        let into = d.join("carried");
        assert_eq!(unpack_into(&again, &into).unwrap(), 5, "and the reader reads it, key frames and all");
        assert_eq!(std::fs::read(into.join("gpu/vk/a.cpp")).unwrap(), b"gpu");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_changed_member_or_other_settings_are_compressed_again() {
        let d = scratch("reuse-changed");
        let (first, _, _) = release_pack(&d, &a_tree(b"gpu"), "zstd 1; -19", &[], 2);
        let old = reusable(&first).unwrap();
        let changed = a_tree(b"gpu, changed");
        let (got, w, calls) = release_pack(&d, &changed, "zstd 1; -19", &old, 2);
        assert_eq!((calls, w.reused), (1, 4), "the changed member alone");
        assert_eq!(got, release_pack(&d, &changed, "zstd 1; -19", &[], 2).0, "the bytes of a fresh pack");
        // (another zstd, or other arguments: nothing taken)
        let (_, w, calls) = release_pack(&d, &a_tree(b"gpu"), "zstd 2; -19", &old, 2);
        assert_eq!((calls, w.reused), (5, 0));
        // (a member whose key frame names this one's raw bytes but whose frame is another's: refused)
        let mut lying = old.clone();
        let (frame, key) = split_key(&lying[1].1).unwrap();
        let mut other = fast_zstd(&member_bytes(&entries(&[("gpu/vk/a.cpp", b"else")])).unwrap()).unwrap();
        assert_ne!(other, frame);
        other.extend(key_frame(&key));
        lying[1].1 = other;
        let (got, w, calls) = release_pack(&d, &a_tree(b"gpu"), "zstd 1; -19", &lying, 2);
        assert_eq!((calls, w.reused, got), (1, 4, first));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn members_made_at_once_give_the_bytes_of_one_after_another() {
        let d = scratch("pool");
        let tree = a_tree(b"gpu");
        let (one, ..) = release_pack(&d, &tree, "s", &[], 1);
        for jobs in [2, 3, 8, 64] {
            assert_eq!(release_pack(&d, &tree, "s", &[], jobs).0, one, "{jobs} at once");
        }
        let out = d.join("plain.pack");
        let w = write(&out, &tree, &[], &fast_zstd).unwrap();
        let plain = std::fs::read(&out).unwrap();
        assert!(reusable(&plain).unwrap().iter().all(|(_, b)| split_key(b).is_none()), "no key frames for this PC's");
        assert_eq!((w.members, w.reused), (5, 0));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_zstd_program_gives_the_same_frame_each_time() {
        if crate::tools::archive::zstd().is_err() {
            eprintln!("no zstd program: skipped");
            return;
        }
        let raw = member_bytes(&[Entry { path: "a".into(), mode: 0, body: Body::Bytes(vec![7; 100_000]) }]).unwrap();
        let one = compress(&raw).unwrap();
        assert_eq!(compress(&raw).unwrap(), one);
        let mut back = Vec::new();
        ruzstd::decoding::StreamingDecoder::new(&one[..]).unwrap().read_to_end(&mut back).unwrap();
        assert_eq!(back, raw);
    }

    #[cfg(unix)]
    #[test]
    fn a_plain_unpack_writes_nothing_through_a_link() {
        // (tests/test_unpack_links.py's, which checked the Python unpacker this replaces)
        let d = scratch("plain-links");
        let hostile = member_modes(&[("a/b", 2, b".."), ("a/b/c", 2, b".."), ("a/b/c/escaped", 0, b"x")]);
        assert!(unpack_into(&pack(&[("tree/x", hostile)]), &d.join("h")).is_err());
        assert!(!d.join("escaped").exists() && !d.parent().unwrap().join("escaped").exists());
        let over = member_modes(&[("a/l", 2, b"x"), ("a/l", 0, b"y")]);
        assert!(unpack_into(&pack(&[("tree/x", over)]), &d.join("o")).is_err());
        let fine = member_modes(&[("usr/lib/a.so", 0, b"a"), ("lib", 2, b"usr/lib")]);
        assert_eq!(unpack_into(&pack(&[("tree/x", fine)]), &d.join("f")).unwrap(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The smallest 64-bit ELF this reads: its header, one program header naming bytes up to `len`, no sections.
    fn tiny_elf(len: usize) -> Vec<u8> {
        let mut f = vec![0u8; len];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 2;
        f[5] = 1;
        f[0x20..0x28].copy_from_slice(&64u64.to_le_bytes()); // (e_phoff)
        f[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); // (e_phentsize)
        f[0x38..0x3A].copy_from_slice(&1u16.to_le_bytes()); // (e_phnum)
        f[64 + 32..64 + 40].copy_from_slice(&(len as u64).to_le_bytes()); // (p_filesz from p_offset 0)
        f
    }

    fn a_pack() -> Vec<u8> {
        pack(&[("tree/files", member(&[("VERSION", b"v9.9.9\n")]))])
    }

    #[test]
    fn an_attached_pack_is_read_back_whole() {
        let program = tiny_elf(200);
        assert_eq!(image_end(&program), Some(200));
        let p = a_pack();
        let file = attach(&program, &p).unwrap();
        assert_eq!(file.len(), 200 + p.len() + TRAILER_LEN);
        let (range, sha) = carried_in(&file).unwrap().unwrap();
        assert_eq!(range, 200..200 + p.len(), "the pack right after the image, the trailer after it");
        assert_eq!(&sha[..], Sha256::digest(&p).as_slice());
        assert_eq!(verify(&file).unwrap(), &p[..]);
        assert_eq!(carried_release(verify(&file).unwrap()).unwrap().0, "v9.9.9");
        assert!(attach(&file, &p).is_err(), "a program that carries a pack already is refused");
        assert!(attach(&program, b"not a pack").is_err());
        assert!(attach(b"neither ELF nor PE", &p).is_err());
    }

    #[test]
    fn a_program_with_no_trailer_carries_nothing() {
        assert_eq!(carried_in(&tiny_elf(200)).unwrap(), None);
        assert_eq!(carried_in(b"").unwrap(), None);
        // (this test's own program, linked as the builder is: its headers name every byte of it)
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert_eq!(image_end(&me), Some(me.len()));
        assert_eq!(carried_in(&me).unwrap(), None);
        assert!(verify(&tiny_elf(200)).is_err());
    }

    #[test]
    fn a_wrong_trailer_is_refused() {
        let file = attach(&tiny_elf(200), &a_pack()).unwrap();
        let end = file.len() - TRAILER_LEN;
        let mut magic = file.clone();
        magic[end] ^= 1;
        assert!(carried_in(&magic).is_err(), "bytes past the image but no trailer: damaged, not a developer's build");
        let mut version = file.clone();
        version[end + 8] = 9;
        assert!(carried_in(&version).is_err());
        let mut offset = file.clone();
        offset[end + 16] ^= 1;
        assert!(carried_in(&offset).is_err(), "a pack that does not end where the trailer starts");
    }

    #[test]
    fn a_truncated_program_is_refused() {
        let file = attach(&tiny_elf(200), &a_pack()).unwrap();
        for cut in [1, TRAILER_LEN / 2, TRAILER_LEN, TRAILER_LEN + 10, file.len() - 199, file.len() - 150] {
            let short = &file[..file.len() - cut];
            assert!(carried_in(short).is_err(), "cut by {cut}");
        }
    }

    #[test]
    fn a_pack_whose_hash_is_wrong_is_refused() {
        let mut file = attach(&tiny_elf(200), &a_pack()).unwrap();
        assert!(carried_in(&file).unwrap().is_some(), "the trailer alone does not hash the pack");
        let last = file.len() - TRAILER_LEN - 1;
        file[last] ^= 0xff;
        let e = verify(&file).unwrap_err().to_string();
        assert!(e.contains("SHA-256") && e.contains("Download longshot-builder again"), "{e}");
    }

    #[test]
    fn a_pack_is_found_appended_or_compiled_in() {
        let p = pack(&[
            ("tree/files", member(&[("VERSION", b"v9.9.9\n")])),
            ("tree/third_party_objects", member(&[("third_party_objects/set.json", b"{}")])),
        ]);
        let appended = attach(&tiny_elf(200), &p).unwrap();
        assert_eq!(find_pack(&appended), Some(&p[..]));
        // (an earlier release's program: the pack among its bytes, a stray magic before it)
        let mut old = tiny_elf(200);
        old.extend_from_slice(PACK_MAGIC);
        old.extend_from_slice(b"junk");
        old.extend_from_slice(&p);
        old.extend_from_slice(b"the program's other sections");
        assert_eq!(find_pack(&old), Some(&p[..]));
        assert_eq!(find_pack(&tiny_elf(200)), None);
        let d = scratch("one-member");
        assert_eq!(unpack_member(&p, "tree/third_party_objects", &d).unwrap(), 1);
        assert_eq!(std::fs::read(d.join("third_party_objects/set.json")).unwrap(), b"{}");
        assert!(!d.join("VERSION").exists(), "that member alone");
        assert!(unpack_member(&p, "tree/nothing", &d).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A PE `len` bytes long with one section of 0x200 bytes at 0x200, no symbols, its FileAlignment 0x200.
    fn tiny_pe(len: usize) -> Vec<u8> {
        let mut f = vec![0u8; len];
        f[..2].copy_from_slice(b"MZ");
        f[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        f[0x80..0x84].copy_from_slice(b"PE\0\0");
        f[0x86..0x88].copy_from_slice(&1u16.to_le_bytes()); // (sections)
        f[0x94..0x96].copy_from_slice(&0xF0u16.to_le_bytes()); // (the optional header's size)
        f[0x98 + 36..0x98 + 40].copy_from_slice(&0x200u32.to_le_bytes()); // (FileAlignment)
        f[0x98 + 60..0x98 + 64].copy_from_slice(&0x200u32.to_le_bytes()); // (SizeOfHeaders)
        let s = 0x98 + 0xF0;
        f[s + 16..s + 20].copy_from_slice(&0x200u32.to_le_bytes());
        f[s + 20..s + 24].copy_from_slice(&0x200u32.to_le_bytes());
        f
    }

    /// [`tiny_pe`] as llvm-mingw's lld links a program that keeps its COFF symbols (a developer's build): the symbols
    /// and their string table after the section (to 0x431), then zeros up to the FileAlignment (0x600).
    fn padded_pe() -> Vec<u8> {
        let mut f = tiny_pe(0x600);
        f[0x8C..0x90].copy_from_slice(&0x400u32.to_le_bytes()); // (PointerToSymbolTable)
        f[0x90..0x94].copy_from_slice(&2u32.to_le_bytes()); // (NumberOfSymbols: 36 bytes)
        f[0x424..0x428].copy_from_slice(&13u32.to_le_bytes()); // (the string table's size, with these 4 bytes)
        f[0x428..0x431].copy_from_slice(b"longshot\0");
        f
    }

    #[test]
    fn a_pe_program_s_overlay_is_past_its_image() {
        let f = tiny_pe(0x400);
        assert_eq!(image_end(&f), Some(0x400));
        let file = attach(&f, &a_pack()).unwrap();
        assert_eq!(carried_in(&file).unwrap().unwrap().0.start, 0x400);
        assert!(carried_in(&file[..file.len() - 3]).is_err());
    }

    #[test]
    fn a_pe_program_s_zero_padding_to_its_file_alignment_carries_nothing() {
        // (a developer's build on Windows: not a download cut short)
        let f = padded_pe();
        assert_eq!(image_end(&f), Some(0x600), "the zeros after the string table are the image's");
        assert_eq!(carried_in(&f).unwrap(), None);
        assert!(carried_in(&f[..0x500]).is_err(), "padding the file has not whole: cut short");
    }

    #[test]
    fn a_pe_program_s_padding_with_a_byte_that_is_not_zero_is_refused() {
        for at in [0x431, 0x5FF] {
            let mut f = padded_pe();
            f[at] = 1;
            assert_eq!(image_end(&f), Some(0x431), "a byte at {at:#x}: past the image");
            assert!(carried_in(&f).is_err(), "a byte at {at:#x}: bytes past the image but no trailer");
            assert!(attach(&f, &a_pack()).is_err(), "a byte at {at:#x}");
        }
    }

    #[test]
    fn a_pack_is_attached_to_a_padded_pe_program() {
        let p = a_pack();
        let file = attach(&padded_pe(), &p).unwrap();
        assert_eq!(carried_in(&file).unwrap().unwrap().0, 0x600..0x600 + p.len(), "the pack right after the padding");
        assert_eq!(verify(&file).unwrap(), &p[..]);
        assert!(attach(&file, &p).is_err(), "a program that carries a pack already is refused");
        // (a program whose string table ends the file: the pack's first bytes are not taken for padding)
        let file = attach(&padded_pe()[..0x431], &p).unwrap();
        assert_eq!(carried_in(&file).unwrap().unwrap().0, 0x431..0x431 + p.len());
        assert_eq!(verify(&file).unwrap(), &p[..]);
    }
}

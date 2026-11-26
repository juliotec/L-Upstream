//! The .nup package: a decrypted Switch 2 dump, its titles each with an exefs and a romfs tree.
//!
//! The layout (worked out from the file; little-endian):
//!
//! ```text
//! 0x000  "NUP\x01"; u64 TOC offset at +8, u64 TOC size at +0x10, the TOC's SHA-256 at +0x18, u64 data offset at +0x38,
//!        u64 file size at +0x40
//! 0x080  "TOC\x01", u32 titles, u64 dirs, u64 files, u64 strings offset (from 0x80), u64 strings size
//! 0x0b0  titles, 0x40 each: u64 title id, u32 root dir, u32 exefs dir, u32 romfs dir, u32 version, ...,
//!        u64 offset + u64 size of the title's metadata at +0x20 (crate::title)
//! then   dirs, 24 bytes each: u32 name offset, u32 name length, u32 first child, u32 children, u64 0
//! then   files, 80 bytes each: u32 name offset, u32 name length, u64 size, u64 data offset, u64 stored size,
//!        32-byte SHA-256 of the file, u8 codec (0 raw, 1 zstd), u8 flags, u16 reserved, 12 bytes reserved
//! then   the child list, u32 each: a node index; below the dir count a dir, else (index - dirs) a file
//! then   the name strings (not 0-terminated)
//! ```
//!
//! A file's data is stored as it is at its offset, or (codec 1, and in packages from older dumpers unmarked) as a zstd
//! frame whose content is the file: its size field is then the file's. A romfs `.zs` file is zstd in the game too: it
//! is one of those only when its frame's content is again a zstd frame ([`Nup::is_packed`]).

use crate::zstd::{MAGIC as ZSTD, frame_content_size, starts_as_zstd};
use build_core::{Error, IoContext, Result};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A title's tree that is not there.
pub const NONE: u32 = 0xffff_ffff;

/// A title of the package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Title {
    /// Its index in the package.
    pub index: usize,
    /// The title id.
    pub id: u64,
    /// Its root directory.
    pub root: u32,
    /// Its exefs directory, or [`NONE`].
    pub exefs: u32,
    /// Its romfs directory, or [`NONE`].
    pub romfs: u32,
    /// Its version (the update's is higher than the base's).
    pub version: u32,
    /// Where its metadata is.
    pub meta: (u64, u64),
}

/// A file's record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    name: (u32, u32),
    /// The file's size.
    pub size: u64,
    /// Where its data is.
    pub offset: u64,
    /// Its data's size in the package.
    pub stored: u64,
    /// The file's SHA-256 (zeros: none given).
    pub hash: [u8; 32],
    /// 0 raw, 1 a zstd frame of the file.
    pub codec: u8,
    /// Flags (none known).
    pub flags: u8,
    /// The reserved bytes (zero).
    pub reserved: [u8; 14],
}

/// Which part of a title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The code modules.
    Exefs,
    /// The game's files.
    Romfs,
}

impl Part {
    /// Its folder's name.
    pub fn name(self) -> &'static str {
        match self {
            Part::Exefs => "exefs",
            Part::Romfs => "romfs",
        }
    }

    fn dir(self, t: &Title) -> u32 {
        match self {
            Part::Exefs => t.exefs,
            Part::Romfs => t.romfs,
        }
    }
}

/// An open package.
#[derive(Debug)]
pub struct Nup {
    path: PathBuf,
    file: File,
    /// The file's size on disk.
    pub len: u64,
    /// The header's file size.
    pub declared_size: u64,
    toc: (u64, u64),
    /// The TOC's SHA-256, as the header gives it: what identifies the package.
    pub toc_sha256: [u8; 32],
    data_offset: u64,
    /// The titles.
    pub titles: Vec<Title>,
    dirs: Vec<[u32; 4]>,
    /// Every file's record.
    pub files: Vec<Record>,
    kids: Vec<u32>,
    strings: Vec<u8>,
}

use crate::le::{le32, le64};

/// `buf.len()` bytes of `file` from `offset` (fewer at its end), without moving a cursor: any thread may read.
pub(crate) fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut buf[done..], offset + done as u64);
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, &mut buf[done..], offset + done as u64);
        match n {
            Ok(0) => break,
            Ok(k) => done += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

/// `buf.len()` bytes of the package from `at`, or the player told why not (the nup example's frames).
pub fn read_exact(nup: &Nup, buf: &mut [u8], at: u64) -> Result<()> {
    nup.read_exact_at(buf, at)
}

/// Windows' errors for a read another program's lock refused: ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
const LOCKED: [i32; 2] = [32, 33];

/// Windows' errors for a drive that cannot give back what it holds.
fn drive_error(code: i32) -> Option<&'static str> {
    Some(match code {
        21 => "the drive is not ready (unplugged, or asleep)",
        23 => "a read failed its data check",
        27 => "the drive could not find a sector",
        30 => "the drive could not read",
        31 => "the drive stopped responding",
        1117 => "the drive reported an I/O error",
        1392 => "the file is corrupted and unreadable",
        _ => return None,
    })
}

/// What to say when the package could not be read at byte `at`: in plain words, with what to do.
pub fn unreadable(path: &Path, at: u64, size: Option<u64>, error: &std::io::Error) -> Error {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let where_ = match size {
        Some(s) => format!("{name} at {} of {} MB", at >> 20, s >> 20),
        None => format!("{name} at {} MB", at >> 20),
    };
    let windows = cfg!(windows).then(|| error.raw_os_error()).flatten();
    let said = match windows {
        Some(c) => format!(" (Windows error {c})"),
        None => format!(" ({error})"),
    };
    if windows.is_some_and(|c| LOCKED.contains(&c)) {
        return Error::player(
            "nup_locked",
            format!(
                "Windows would not let the build read {where_}: another program has that part of the file locked{said}. A \
                 backup or sync program, or a virus scan, can do this. Close it and build again, or copy the .nup to another \
                 folder and build from the copy."
            ),
        );
    }
    let drive = windows.and_then(drive_error);
    // (EIO, 5 on Linux: the drive's own error)
    if drive.is_some() || (windows.is_none() && error.raw_os_error() == Some(5)) {
        return Error::player(
            "nup_drive_error",
            format!(
                "Your drive could not read {where_}: {}{said}. Copy the .nup to another drive and build from the copy. If \
                 the copy fails too, that part of the drive is damaged: check the drive for errors, or dump the game again \
                 onto another drive.",
                drive.unwrap_or("the drive reported an I/O error")
            ),
        );
    }
    Error::player(
        "nup_unreadable",
        format!(
            "The build could not read {where_}{said}. Close any program that may be using the file and build again, or \
             copy the .nup to another drive and build from the copy."
        ),
    )
}

/// The package's bytes from an offset, as a reader (positional: many may read one package at once).
pub(crate) struct Region<'a> {
    nup: &'a Nup,
    at: u64,
    left: u64,
}

impl Region<'_> {
    pub(crate) fn left(&self) -> u64 {
        self.left
    }
}

impl Read for Region<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let got = read_at(&self.nup.file, &mut buf[..n], self.at)?;
        self.at += got as u64;
        self.left -= got as u64;
        Ok(got)
    }
}

/// The ways copying a file out can fail: the package's, or the sink's.
enum CopyError {
    /// The package is damaged or unreadable there (said for the player).
    Package(Error),
    /// The sink refused the bytes.
    Sink(std::io::Error),
}

impl From<Error> for CopyError {
    fn from(e: Error) -> CopyError {
        CopyError::Package(e)
    }
}

impl Nup {
    /// The package at `path`, its tables read and checked (every range in its tables, every file reachable from a
    /// title and no directory inside itself).
    pub fn open(path: &Path) -> Result<Nup> {
        let file = File::open(path).map_err(|e| {
            Error::player(
                "nup_missing",
                format!(
                    "Could not open your .nup file {}: {e}. Check that the file is still there, reconnect its drive if \
                     needed, or choose the file again.",
                    path.display()
                ),
            )
        })?;
        let len = file.metadata().at(path)?.len();
        let mut nup = Nup {
            path: path.to_path_buf(),
            file,
            len,
            declared_size: 0,
            toc: (0, 0),
            toc_sha256: [0; 32],
            data_offset: 0,
            titles: Vec::new(),
            dirs: Vec::new(),
            files: Vec::new(),
            kids: Vec::new(),
            strings: Vec::new(),
        };
        let mut h = [0u8; 0x80];
        if nup.read_exact_at(&mut h, 0).is_err() || h[..4] != *b"NUP\x01" {
            return Err(Error::player(
                "not_a_nup",
                format!("{} is not a .nup package. Choose the .nup file your dumper wrote.", path.display()),
            ));
        }
        nup.declared_size = le64(&h, 0x40);
        nup.toc = (le64(&h, 8), le64(&h, 0x10));
        nup.toc_sha256.copy_from_slice(&h[24..56]);
        nup.data_offset = le64(&h, 0x38);
        // (a package that ends early, a download or copy that did not finish, failed 30 minutes in as a short read: said
        // now, with the sizes)
        if nup.declared_size != 0 && len < nup.declared_size {
            return Err(Error::player(
                "nup_incomplete",
                format!(
                    "The .nup file is incomplete: it is {} MB, its header says {} MB. Download or copy it again (wait for the \
                     download to finish) and check that the drive had room.",
                    len >> 20,
                    nup.declared_size >> 20
                ),
            ));
        }
        nup.read_tables().map_err(|d| Error::format(path.display().to_string(), d))?;
        Ok(nup)
    }

    /// The package's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_exact_at(&self, buf: &mut [u8], at: u64) -> Result<()> {
        match read_at(&self.file, buf, at) {
            Ok(n) if n == buf.len() => Ok(()),
            Ok(_) => Err(Error::player(
                "nup_incomplete",
                format!(
                    "The .nup file ends early (a short read at {} of {} MB): it is incomplete. Download or copy it again.",
                    at >> 20,
                    self.len >> 20
                ),
            )),
            Err(e) => Err(unreadable(&self.path, at, Some(self.len), &e)),
        }
    }

    fn read_tables(&mut self) -> std::result::Result<(), String> {
        let mut t = [0u8; 0x30];
        self.read_exact_at(&mut t, 0x80).map_err(|e| e.to_string())?;
        if t[..4] != *b"TOC\x01" {
            return Err("no TOC at 0x80".into());
        }
        let (nt, nd, nf, soff, ssz) = (u64::from(le32(&t, 4)), le64(&t, 8), le64(&t, 16), le64(&t, 24), le64(&t, 32));
        let tables_end = 0x30u64
            .checked_add(nt.checked_mul(64).ok_or("too many titles")?)
            .and_then(|v| v.checked_add(nd.checked_mul(24)?))
            .and_then(|v| v.checked_add(nf.checked_mul(80)?))
            .ok_or("tables too large")?;
        let strings_end = soff.checked_add(ssz).ok_or("strings too large")?;
        let meta_end = strings_end.checked_add(0x80).ok_or("strings too large")?;
        if nt == 0 || tables_end > soff || (soff - tables_end) % 4 != 0 || meta_end > self.len {
            return Err("the title, directory, file, child and string tables exceed the file".into());
        }
        let mut meta = vec![0u8; usize::try_from(meta_end).map_err(|_| "tables too large")?];
        self.read_exact_at(&mut meta, 0).map_err(|e| e.to_string())?;
        let (dirs_at, files_at) = (0xb0 + 0x40 * nt as usize, 0xb0 + 0x40 * nt as usize + 24 * nd as usize);
        let (kids_at, strings_at) = (files_at + 80 * nf as usize, 0x80 + soff as usize);
        self.titles = (0..nt as usize)
            .map(|i| {
                let o = 0xb0 + 0x40 * i;
                Title {
                    index: i,
                    id: le64(&meta, o),
                    root: le32(&meta, o + 8),
                    exefs: le32(&meta, o + 12),
                    romfs: le32(&meta, o + 16),
                    version: le32(&meta, o + 20),
                    meta: (le64(&meta, o + 0x20), le64(&meta, o + 0x28)),
                }
            })
            .collect();
        self.dirs = (0..nd as usize)
            .map(|i| {
                let o = dirs_at + 24 * i;
                [le32(&meta, o), le32(&meta, o + 4), le32(&meta, o + 8), le32(&meta, o + 12)]
            })
            .collect();
        self.files = (0..nf as usize)
            .map(|i| {
                let o = files_at + 80 * i;
                Record {
                    name: (le32(&meta, o), le32(&meta, o + 4)),
                    size: le64(&meta, o + 8),
                    offset: le64(&meta, o + 16),
                    stored: le64(&meta, o + 24),
                    hash: meta[o + 32..o + 64].try_into().unwrap_or_default(),
                    codec: meta[o + 64],
                    flags: meta[o + 65],
                    reserved: meta[o + 66..o + 80].try_into().unwrap_or_default(),
                }
            })
            .collect();
        self.kids = meta[kids_at..strings_at].as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        self.strings = meta[strings_at..].to_vec();
        self.check_tree()
    }

    /// Every range in range, every node reachable from a title's root, no directory inside itself (without recursion:
    /// a deep tree must not overflow the stack).
    fn check_tree(&self) -> std::result::Result<(), String> {
        let (nd, nf) = (self.dirs.len(), self.files.len());
        for &[no, nl, first, count] in &self.dirs {
            if self.name_of(no, nl).is_none() || first as usize + count as usize > self.kids.len() {
                return Err("a directory's name or children are outside their tables".into());
            }
        }
        if self.files.iter().any(|r| self.name_of(r.name.0, r.name.1).is_none()) {
            return Err("a file's name is outside the string table".into());
        }
        // (each file's data inside the package: as it is (codec 0, its size), or its frame (codec 1, its stored
        // size); an extent that wraps or ends past the file would read nothing, or past the end, as if whole)
        for r in &self.files {
            let extent = if r.codec == 1 { r.stored } else { r.size };
            if r.size > 0 && r.offset.checked_add(extent).is_none_or(|end| end > self.len) {
                return Err(format!("the data of {} is outside the package", self.name(r)));
            }
        }
        if self.kids.iter().any(|&k| k as usize >= nd + nf) {
            return Err("a child is outside the directory and file tables".into());
        }
        // (each directory and file the child of one directory at most: a subtree shared by two parents is walked, and
        // its paths listed, once for each, so a few shared levels make the listing exponential; stopped at the first
        // second parent, so this costs at most one count per entry)
        let mut parent = vec![false; nd + nf];
        for &[_, _, first, count] in &self.dirs {
            for &k in &self.kids[first as usize..first as usize + count as usize] {
                if std::mem::replace(&mut parent[k as usize], true) {
                    return Err("a directory or file is the child of two directories".into());
                }
            }
        }
        let (mut reached_dirs, mut reached_files) = (vec![false; nd], vec![false; nf]);
        for t in &self.titles {
            let part_ok = |d: u32| d == NONE || (d as usize) < nd;
            if t.root as usize >= nd || !part_ok(t.exefs) || !part_ok(t.romfs) {
                return Err(format!("title {} has an invalid directory index", t.index));
            }
            let (mut seen, mut active) = (vec![false; nd], vec![false; nd]);
            let mut pending = vec![(t.root, false)];
            while let Some((d, leaving)) = pending.pop() {
                let d = d as usize;
                if leaving {
                    active[d] = false;
                    seen[d] = true;
                    continue;
                }
                if active[d] {
                    return Err(format!("a directory inside itself in title {}", t.index));
                }
                if seen[d] {
                    continue;
                }
                active[d] = true;
                pending.push((d as u32, true));
                let [_, _, first, count] = self.dirs[d];
                for &k in &self.kids[first as usize..first as usize + count as usize] {
                    if (k as usize) < nd {
                        pending.push((k, false));
                    } else {
                        reached_files[k as usize - nd] = true;
                    }
                }
            }
            if [t.exefs, t.romfs].iter().any(|&p| p != NONE && !seen[p as usize]) {
                return Err("a title's exefs or romfs is not inside its root".into());
            }
            for (r, s) in reached_dirs.iter_mut().zip(&seen) {
                *r |= *s;
            }
        }
        if reached_dirs.contains(&false) || reached_files.contains(&false) {
            return Err("directories or files not reachable from any title".into());
        }
        Ok(())
    }

    fn name_of(&self, off: u32, len: u32) -> Option<&str> {
        std::str::from_utf8(self.strings.get(off as usize..off as usize + len as usize)?).ok()
    }

    /// A file's name.
    pub fn name(&self, r: &Record) -> &str {
        self.name_of(r.name.0, r.name.1).unwrap_or_default()
    }

    /// Every file under directory `dir`: its path from there ("/" between names), its record's index, in the tree's
    /// order.
    pub fn walk(&self, dir: u32) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        if dir == NONE {
            return out;
        }
        let nd = self.dirs.len();
        let mut pending = vec![(dir as usize, String::new())];
        while let Some((node, prefix)) = pending.pop() {
            if node < nd {
                let [_, _, first, count] = self.dirs[node];
                for &k in self.kids[first as usize..first as usize + count as usize].iter().rev() {
                    let k = k as usize;
                    let path = if k < nd {
                        let [no, nl, _, _] = self.dirs[k];
                        format!("{prefix}{}/", self.name_of(no, nl).unwrap_or_default())
                    } else {
                        prefix.clone()
                    };
                    pending.push((k, path));
                }
            } else {
                let i = node - nd;
                out.push((format!("{prefix}{}", self.name(&self.files[i])), i));
            }
        }
        out
    }

    /// The files of a title's part.
    pub fn part(&self, t: &Title, part: Part) -> Vec<(String, usize)> {
        self.walk(part.dir(t))
    }

    /// The title `want` names (its index, version or title id), or the newest (the update over the base).
    pub fn pick_title(&self, want: Option<&str>) -> Result<&Title> {
        let Some(want) = want else {
            return Ok(self.titles.iter().max_by_key(|t| t.version).unwrap_or(&self.titles[0]));
        };
        self.titles
            .iter()
            .find(|t| {
                t.index.to_string() == want
                    || t.version.to_string() == want
                    || format!("{:016x}", t.id) == want.to_lowercase()
            })
            .ok_or_else(|| Error::player("no_such_title", format!("The .nup has no title {want}.")))
    }

    /// The package's bytes from `at`, `len` of them, as a reader.
    pub(crate) fn region(&self, at: u64, len: u64) -> Region<'_> {
        Region { nup: self, at, left: len }
    }

    fn read_err(&self, e: std::io::Error, at: u64) -> Error {
        unreadable(&self.path, at, Some(self.len), &e)
    }

    /// Whether the package keeps this file compressed. Marked (codec 1), or older dumpers' unmarked frames: a frame
    /// that states no size is tried, and is the file when it unpacks to the file's size. A `.zs` file is zstd in the
    /// game too, and some happen to hold exactly as much as they take: that one is packed only if what it holds is
    /// zstd again.
    pub fn is_packed(&self, r: &Record, name: &str) -> Result<bool> {
        if r.codec == 1 {
            return Ok(true);
        }
        let mut head = [0u8; 18];
        let n = read_at(&self.file, &mut head, r.offset).map_err(|e| self.read_err(e, r.offset))?;
        let head = &head[..n.min(r.size as usize)];
        if head.len() < 4 || head[..4] != ZSTD {
            return Ok(false);
        }
        let size = frame_content_size(head);
        if name.ends_with(".zs") && size.is_none_or(|s| s == r.size) {
            return Ok(size == Some(r.size) && self.inner_is_zstd(r));
        }
        if let Some(s) = size {
            return Ok(s == r.size);
        }
        let mut sink = Counter(0);
        Ok(self.decode(r, r.size + (1 << 16), &mut sink, false).is_ok() && sink.0 == r.size)
    }

    /// Whether what this file's frame holds starts as a zstd frame.
    fn inner_is_zstd(&self, r: &Record) -> bool {
        self.frame_head(r.offset, r.size.min(r.stored.max(r.size)), 4).is_ok_and(|h| h.len() >= 4 && h[..4] == ZSTD)
    }

    /// The zstd frame at the record's offset, from at most `left` bytes, into `sink`. `exact`: the frame must take all
    /// of them and hold the whole file.
    /// The frame of `r` (in its first `left` bytes) decoded into `sink`: the bytes it made, and the frame's length.
    fn decode(
        &self,
        r: &Record,
        left: u64,
        sink: &mut dyn Write,
        exact: bool,
    ) -> std::result::Result<(u64, u64), CopyError> {
        let damaged = |what: String| CopyError::Package(Error::format(self.path.display().to_string(), what));
        let mut src = self.region(r.offset, left);
        let mut dec = ruzstd::decoding::FrameDecoder::new();
        dec.reset(&mut src).map_err(|e| damaged(format!("a compressed file's frame: {e}")))?;
        let mut got = 0u64;
        loop {
            let finished = dec
                .decode_blocks(&mut src, ruzstd::decoding::BlockDecodingStrategy::UptoBytes(8 << 20))
                .map_err(|e| damaged(format!("a compressed file's frame: {e}")))?;
            let n = dec.collect_to_writer(&mut *sink).map_err(CopyError::Sink)? as u64;
            got += n;
            if got > r.size {
                return Err(damaged(format!("a compressed file expanded past its declared size of {} bytes", r.size)));
            }
            if finished && dec.can_collect() == 0 {
                break;
            }
        }
        if let (Some(want), Some(have)) = (dec.get_checksum_from_data(), dec.get_calculated_checksum())
            && want != have
        {
            return Err(damaged("a compressed file's checksum is wrong".into()));
        }
        if exact && src.left() != 0 {
            return Err(damaged("a compressed extent has trailing bytes".into()));
        }
        if got != r.size {
            return Err(damaged(format!("a compressed file came out as {got} bytes, {} expected", r.size)));
        }
        Ok((got, left - src.left()))
    }

    /// The file as itself into `sink`. `packed`: how it is stored, when known (else [`Nup::is_packed`] decides).
    fn copy(
        &self,
        r: &Record,
        name: &str,
        sink: &mut dyn Write,
        packed: Option<bool>,
    ) -> std::result::Result<(), CopyError> {
        let exact = packed.is_some();
        let packed = match packed {
            Some(p) => p,
            None => self.is_packed(r, name)?,
        };
        if packed {
            // (marked extents must not read into the next member; an unmarked frame is read until it ends)
            let left = if r.codec == 1 { r.stored } else { r.size.saturating_add(1 << 16) };
            return self.decode(r, left, sink, exact).map(|_| ());
        }
        let mut buf = vec![0u8; (8 << 20).min(r.size) as usize];
        let mut at = r.offset;
        let end = r.offset.saturating_add(r.size); // (inside the package: Nup::open checked every extent)
        while at < end {
            let n = buf.len().min((end - at) as usize);
            self.read_exact_at(&mut buf[..n], at)?;
            sink.write_all(&buf[..n]).map_err(CopyError::Sink)?;
            at += n as u64;
        }
        Ok(())
    }

    /// A whole file's bytes.
    pub fn read(&self, index: usize) -> Result<Vec<u8>> {
        let r = &self.files[index];
        // (whole in memory: a code module's most, no file of the package the builder reads whole is larger)
        if r.size > crate::nso::MAX_MODULE as u64 {
            return Err(Error::format(
                self.path.display().to_string(),
                format!("{} says it is {} bytes: larger than any file the builder reads whole", self.name(r), r.size),
            ));
        }
        // (a record's size reserved only up to 64 MB: the bytes decoded say how much more it needs)
        let mut out = Vec::with_capacity(r.size.min(64 << 20) as usize);
        match self.copy(r, self.name(r), &mut out, None) {
            Ok(()) => Ok(out),
            Err(CopyError::Package(e)) => Err(e),
            Err(CopyError::Sink(e)) => Err(Error::io(&self.path, e)),
        }
    }

    /// The open file (positional reads only).
    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    /// The length of the zstd frame a packed file is stored as (for an unmarked frame, found by decoding it).
    pub fn frame_length(&self, index: usize) -> Result<u64> {
        let r = &self.files[index];
        if r.codec == 1 {
            return Ok(r.stored);
        }
        match self.decode(r, r.size.saturating_add(1 << 16), &mut std::io::sink(), false) {
            Ok((_, used)) => Ok(used),
            Err(CopyError::Package(e)) => Err(e),
            Err(CopyError::Sink(e)) => Err(Error::io(&self.path, e)),
        }
    }

    /// The first `n` bytes of what the zstd frame at `offset` (of `stored` bytes) holds: the blocks that make them
    /// decoded alone (a block is at most 128 KB: the first few hundred KB of the frame hold them).
    pub(crate) fn frame_head(&self, offset: u64, stored: u64, n: usize) -> Result<Vec<u8>> {
        // (64 KB holds the first block of most; else as much as any first block takes)
        for room in [64u64 << 10, 512 << 10] {
            let mut src = vec![0u8; stored.min(room) as usize];
            let got = read_at(&self.file, &mut src, offset).map_err(|e| self.read_err(e, offset))?;
            src.truncate(got);
            if let Ok(head) = crate::zstd::prefix(crate::zstd::Dialect::Zstd, &src, n) {
                return Ok(head);
            }
            if stored <= room {
                break;
            }
        }
        Ok(Vec::new())
    }

    /// The first `n` bytes of a file (for a raw one, read alone).
    pub fn head(&self, index: usize, n: usize) -> Result<Vec<u8>> {
        let r = &self.files[index];
        if r.codec == 0 && !self.is_packed(r, self.name(r))? {
            let mut b = vec![0u8; (n as u64).min(r.size) as usize];
            self.read_exact_at(&mut b, r.offset)?;
            return Ok(b);
        }
        let stored = if r.codec == 1 { r.stored } else { r.size.saturating_add(1 << 16) };
        self.frame_head(r.offset, stored, n)
    }
}

struct Counter(u64);

impl Write for Counter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0 += b.len() as u64;
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// ---- verify

/// What [`verify`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verified {
    /// File records checked.
    pub checked: usize,
    /// Distinct extents decoded (records may share one).
    pub extents: usize,
    /// One line per bad record, or for the package.
    pub bad: Vec<String>,
}

/// A file's data in the package: offset, stored size, codec, size (records may share one).
type Extent = (u64, u64, u8, u64);
/// What decoding an extent gave: its length, its SHA-256, its first bytes, the error.
type Decoded = (u64, [u8; 32], Vec<u8>, Option<String>);
/// A title's files to verify, with the part they are of.
type Listed<'a> = (&'a Title, &'static str, Vec<(String, usize)>);

struct Digest256 {
    size: u64,
    prefix: Vec<u8>,
    hash: Sha256,
}

impl Write for Digest256 {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.size += b.len() as u64;
        self.hash.update(b);
        let need = 18usize.saturating_sub(self.prefix.len());
        self.prefix.extend_from_slice(&b[..need.min(b.len())]);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The package checked without writing anything: the TOC's hash, each file's extent, decoded length and SHA-256, and
/// each `.zs` file's game zstd header (not their inner data). Every title, or those given; `only`: one part of each.
/// `progress(done, total)` after each file.
pub fn verify(
    nup: &Nup,
    titles: Option<&[usize]>,
    only: Option<Part>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Verified> {
    let mut v = Verified::default();
    if let Err(e) = verify_toc(nup) {
        v.bad.push(format!("BAD package: {e}"));
        return Ok(v);
    }
    let mut cache: std::collections::HashMap<Extent, Decoded> = std::collections::HashMap::new();
    let picked: Vec<&Title> = match titles {
        None => nup.titles.iter().collect(),
        Some(ix) => ix.iter().filter_map(|&i| nup.titles.get(i)).collect(),
    };
    let lists: Vec<Listed> = picked
        .iter()
        .map(|t| match only {
            Some(p) => (*t, p.name(), nup.part(t, p)),
            None => (*t, "", nup.walk(t.root)),
        })
        .collect();
    let total: usize = lists.iter().map(|l| l.2.len()).sum();
    for (t, part, files) in lists {
        for (name, i) in files {
            v.checked += 1;
            let r = &nup.files[i];
            let mut reasons = Vec::new();
            if r.codec > 1 || r.flags != 0 || r.reserved.iter().any(|&b| b != 0) {
                reasons.push("unsupported codec/flags/reserved bytes".to_string());
            }
            if (r.size == 0 && (r.offset != 0 || r.stored != 0 || r.codec != 0))
                || (r.size != 0
                    && (r.stored == 0
                        || r.offset < nup.data_offset
                        || !r.offset.is_multiple_of(16)
                        || r.offset > nup.len
                        || r.stored > nup.len - r.offset))
            {
                reasons.push("invalid extent bounds".into());
            }
            if r.codec == 0 && r.stored != r.size {
                reasons.push("raw extent length differs from member length".into());
            }
            if reasons.is_empty() {
                let key = (r.offset, r.stored, r.codec, r.size);
                let (count, digest, prefix, error) = cache.entry(key).or_insert_with(|| {
                    let mut sink = Digest256 { size: 0, prefix: Vec::new(), hash: Sha256::new() };
                    let error = match nup.copy(r, &name, &mut sink, Some(r.codec == 1)) {
                        Ok(()) => None,
                        Err(CopyError::Package(e)) => Some(e.to_string()),
                        Err(CopyError::Sink(e)) => Some(e.to_string()),
                    };
                    (sink.size, sink.hash.finalize().into(), sink.prefix, error)
                });
                if let Some(e) = error {
                    reasons.push(format!("member decode failed: {e}"));
                }
                if *count != r.size {
                    reasons.push("decoded length differs from member length".into());
                }
                if *digest != r.hash {
                    reasons.push("member SHA-256 mismatch".into());
                }
                if name.ends_with(".zs") && !game_zstd_header(prefix) {
                    reasons.push(".zs member lacks the expected game Zstd header".into());
                }
            }
            if !reasons.is_empty() {
                v.bad.push(format!(
                    "BAD title {} {:016x} v{} {part}/{name}: {}",
                    t.index,
                    t.id,
                    t.version,
                    reasons.join("; ")
                ));
            }
            progress(v.checked, total);
        }
    }
    v.extents = cache.len();
    Ok(v)
}

/// A zstd frame header that parses: the magic, the reserved bit clear, and room for its fields.
fn game_zstd_header(prefix: &[u8]) -> bool {
    if prefix.len() < 5 || prefix[..4] != ZSTD || prefix[4] & 0x08 != 0 {
        return false;
    }
    let fhd = prefix[4];
    let single = fhd & 0x20 != 0;
    let need = 5
        + usize::from(!single)
        + [0, 1, 2, 4][usize::from(fhd & 3)]
        + [usize::from(single), 2, 4, 8][usize::from(fhd >> 6)];
    prefix.len() >= need
}

fn verify_toc(nup: &Nup) -> std::result::Result<(), String> {
    let (off, size) = nup.toc;
    if nup.declared_size != nup.len
        || off != 0x80
        || size < 0x30
        || off.checked_add(size).is_none_or(|end| end > nup.data_offset)
        || nup.data_offset > nup.len
        || !nup.data_offset.is_multiple_of(16)
    {
        return Err("invalid declared file/TOC/blob bounds".into());
    }
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; 8 << 20];
    let mut at = off;
    while at < off + size {
        let n = buf.len().min((off + size - at) as usize);
        nup.read_exact_at(&mut buf[..n], at).map_err(|e| e.to_string())?;
        digest.update(&buf[..n]);
        at += n as u64;
    }
    if digest.finalize()[..] != nup.toc_sha256 {
        return Err("TOC SHA-256 mismatch".into());
    }
    Ok(())
}

// ---- extract

/// Windows' device names: a file of that name (any extension) opens the device, not a file.
fn windows_device(stem: &str) -> bool {
    let s = stem.trim_end_matches(' ').to_uppercase();
    matches!(s.as_str(), "CON" | "CONIN$" | "CONOUT$" | "PRN" | "AUX" | "NUL")
        || (s.len() == 4 || s.chars().count() == 4)
            && (s.starts_with("COM") || s.starts_with("LPT"))
            && s.chars().nth(3).is_some_and(|c| "123456789¹²³".contains(c))
}

/// A package path's names, refused when one could leave its folder or mean something else on Windows: empty, `.`,
/// `..`, a trailing space or dot, `<>:"|?*` or a control character, a device name, or a backslash anywhere.
pub fn package_parts(rel: &str) -> Result<Vec<&str>> {
    let unsafe_part = |p: &str| {
        p.is_empty()
            || p == "."
            || p == ".."
            || p.ends_with([' ', '.'])
            || p.chars().any(|c| "<>:\"|?*".contains(c) || (c as u32) < 32)
            || windows_device(p.split('.').next().unwrap_or_default())
    };
    let parts: Vec<&str> = rel.split('/').collect();
    if rel.is_empty() || rel.contains('\\') || parts.iter().any(|p| unsafe_part(p)) {
        return Err(Error::player(
            "nup_unsafe_path",
            format!("The .nup holds an unsafe path: {rel:?}. Dump the game again."),
        ));
    }
    Ok(parts)
}

use crate::checks::is_link;

fn refuse_link(path: &Path) -> Result<Option<std::fs::Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if is_link(&m) => Err(Error::player(
            "extraction_link",
            format!("The extraction path {} is a link or reparse point: choose another folder.", path.display()),
        )),
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// The folders of an extraction, each checked (not a link) or made once.
struct Destination {
    root: PathBuf,
    folders: std::collections::HashSet<PathBuf>,
}

impl Destination {
    fn new(out: &Path) -> Result<Destination> {
        std::fs::create_dir_all(out).at(out)?;
        let root = std::fs::canonicalize(out).at(out)?;
        Ok(Destination { root, folders: Default::default() })
    }

    fn folder(&mut self, path: &Path) -> Result<()> {
        if !self.folders.contains(path) {
            if refuse_link(path)?.is_none() {
                std::fs::create_dir_all(path).at(path)?;
            }
            self.folders.insert(path.to_path_buf());
        }
        Ok(())
    }

    fn path(&mut self, part: Part, rel: &str) -> Result<PathBuf> {
        let parts = package_parts(rel)?;
        let mut at = self.root.join(part.name());
        self.folder(&at)?;
        for name in &parts[..parts.len() - 1] {
            at.push(name);
            self.folder(&at)?;
        }
        Ok(at.join(parts[parts.len() - 1]))
    }
}

/// What an extraction wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extracted {
    /// Files written now (those already whole were kept).
    pub files: usize,
    /// Their bytes.
    pub bytes: u64,
    /// The romfs `.zs` files that do not start as zstd data or whose SHA-256 is not the package's (paths under the
    /// romfs, "/"): a damaged dump (crate::romfs's stand-ins).
    pub damaged: Vec<String>,
}

/// A full drive, said with the file being written and what the drive reports (a drive can refuse a write with room to
/// spare: the two are told apart).
fn out_of_space(e: &std::io::Error, dst: &Path, rel: &str, size: u64) -> Option<Error> {
    if !build_core::is_disk_full(e) {
        return None;
    }
    let folder = dst.parent().unwrap_or(dst);
    let free = crate::disk::free_bytes(folder)
        .map_or("free space unknown".to_string(), |b| format!("{:.1} GB free there", b as f64 / 1e9));
    let kind = crate::disk::file_system(folder).map(|k| format!(", {k}")).unwrap_or_default();
    Some(Error::player(
        "disk_full",
        format!(
            "The drive is full: no room to write {rel} ({size} bytes) into {} ({free}{kind}). Free some space, or choose \
             a game folder on another drive, and build again.",
            folder.display()
        ),
    ))
}

/// A title's exefs, or its romfs, or both, extracted under `out` (`out/exefs`, `out/romfs`). A file already there whole
/// (its size) is kept, so an extraction stopped half way goes on where it stopped; a shorter one is written again
/// (unlinked first, so a hard link to it keeps its data). Nothing is written through a link. `progress(bytes)` as it
/// writes.
pub fn extract(
    nup: &Nup,
    t: &Title,
    out: &Path,
    only: Option<Part>,
    progress: &mut dyn FnMut(u64),
) -> Result<Extracted> {
    let mut where_ = Destination::new(out)?;
    let mut done = Extracted::default();
    for part in [Part::Exefs, Part::Romfs] {
        if only.is_some_and(|o| o != part) {
            continue;
        }
        for (rel, i) in nup.part(t, part) {
            let r = &nup.files[i];
            let dst = where_.path(part, &rel)?;
            let checked = part == Part::Romfs && rel.ends_with(".zs");
            let expected = (checked && r.hash != [0; 32]).then_some(r.hash);
            if let Some(m) = refuse_link(&dst)? {
                if m.is_file() && m.len() == r.size {
                    if checked && file_damaged(&dst, expected)? {
                        done.damaged.push(rel.clone());
                    }
                    continue;
                }
                std::fs::remove_file(&dst).at(&dst)?;
            }
            let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&dst).at(&dst)?;
            let mut sink = Sink {
                out: std::io::BufWriter::with_capacity(1 << 20, file),
                head: Vec::new(),
                hash: expected.map(|_| Sha256::new()),
                progress,
            };
            let copied = nup.copy(r, &rel, &mut sink, None).and_then(|()| sink.out.flush().map_err(CopyError::Sink));
            if let Err(e) = copied {
                drop(sink);
                let error = match e {
                    CopyError::Sink(io) => out_of_space(&io, &dst, &rel, r.size).unwrap_or_else(|| Error::io(&dst, io)),
                    CopyError::Package(e) => e,
                };
                // (the failed file's space freed, without hiding the error)
                let _ = std::fs::remove_file(&dst);
                return Err(error);
            }
            if checked
                && (!starts_as_zstd(&sink.head)
                    || sink.hash.take().is_some_and(|h| Some(<[u8; 32]>::from(h.finalize())) != expected))
            {
                done.damaged.push(rel.clone());
            }
            done.files += 1;
            done.bytes += r.size;
        }
    }
    Ok(done)
}

struct Sink<'a, W: Write> {
    out: W,
    head: Vec<u8>,
    hash: Option<Sha256>,
    progress: &'a mut dyn FnMut(u64),
}

impl<W: Write> Write for Sink<'_, W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let n = self.out.write(b)?;
        if self.head.len() < 4 {
            self.head.extend_from_slice(&b[..n.min(4 - self.head.len())]);
        }
        if let Some(h) = &mut self.hash {
            h.update(&b[..n]);
        }
        (self.progress)(n as u64);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// Whether a `.zs` file on disk is damaged: it does not start as zstd, or its SHA-256 is not `expected`.
fn file_damaged(path: &Path, expected: Option<[u8; 32]>) -> Result<bool> {
    let mut f = File::open(path).at(path)?;
    let mut head = [0u8; 4];
    let n = read_at(&f, &mut head, 0).at(path)?;
    if !starts_as_zstd(&head[..n]) {
        return Ok(true);
    }
    let Some(want) = expected else { return Ok(false) };
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).at(path)?;
    Ok(<[u8; 32]>::from(h.finalize()) != want)
}

/// An update's exefs need not hold every module whole: a module of `t`'s exefs under `out/exefs` that is not an NSO
/// is taken from the package's other titles (the newest first). The modules taken, each with the title's index.
pub fn heal(nup: &Nup, t: &Title, out: &Path) -> Result<Vec<(String, usize)>> {
    let mut taken = Vec::new();
    for m in crate::nso::MODULE_ORDER {
        let p = out.join("exefs").join(m);
        let mut have = [0u8; 4];
        let n = match File::open(&p) {
            Ok(f) => read_at(&f, &mut have, 0).at(&p)?,
            Err(_) => 0,
        };
        if n == 4 && have == *b"NSO0" {
            continue;
        }
        'titles: for other in nup.titles.iter().rev().filter(|o| o.index != t.index) {
            for (rel, i) in nup.part(other, Part::Exefs) {
                if rel == m && nup.head(i, 4)? == b"NSO0" {
                    build_core::fsx::write_atomic(&p, &nup.read(i)?)?;
                    taken.push((m.to_string(), other.index));
                    break 'titles;
                }
            }
        }
    }
    Ok(taken)
}

#[cfg(test)]
#[path = "nup_tests.rs"]
mod out_of_space_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) use crate::testkit::{NupBuilder as Builder, zstd_frame};

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("game-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    pub(crate) fn sample() -> Builder {
        let module = crate::nso::tests::synthetic(0xe0, true);
        let mut zs = zstd_frame(b"a game file, itself zstd");
        zs.extend_from_slice(&[0; 3]);
        let mut damaged = zs.clone();
        damaged[..4].copy_from_slice(&[0; 4]);
        Builder {
            titles: vec![
                (
                    0x0400486024dec000,
                    0,
                    vec![("main".into(), module.clone(), 0), ("rtld".into(), b"NSO0rtld".to_vec(), 0)],
                    vec![],
                ),
                (
                    0x0400486024dec000,
                    131072,
                    vec![("main".into(), module, 1), ("rtld".into(), b"not a module".to_vec(), 0)],
                    vec![
                        ("Pack/ZsDic.pack.zs".into(), zs.clone(), 1),
                        ("Pack/Other.pack.zs".into(), zs.clone(), 0),
                        ("Pack/Wrapped.zs".into(), zstd_frame(crate::testdata::text()), 1),
                        // (its .nup frame's content, 354 KB, more than a streamed piece: decoded as a stream)
                        (
                            "Pack/WideWrapped.zs".into(),
                            crate::testkit::raw_frame(&crate::testdata::text().repeat(3), 17),
                            2,
                        ),
                        ("Pack/Wide.bin".into(), crate::testdata::text().to_vec(), 2),
                        ("Case/Same.txt".into(), b"two names that differ in case only".to_vec(), 0),
                        ("Case/same.txt".into(), b"the other one".to_vec(), 0),
                        ("Model/Wall_Alb.bntx.zs".into(), damaged, 0),
                        ("Model/Wall2_Alb.bntx.zs".into(), zs, 0),
                        ("System/Resource/empty.txt".into(), Vec::new(), 0),
                        ("big.bin".into(), crate::testdata::text().to_vec(), 1),
                    ],
                ),
            ],
        }
    }

    #[test]
    fn a_package_reads_back() {
        let d = scratch("nup-read");
        let p = d.join("game.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let nup = Nup::open(&p).unwrap();
        assert_eq!(nup.titles.len(), 2);
        let t = nup.pick_title(None).unwrap().clone();
        assert_eq!(t.version, 131072);
        assert_eq!(nup.pick_title(Some("0")).unwrap().index, 0);
        assert!(matches!(nup.pick_title(Some("7")), Err(Error::Player { code: "no_such_title", .. })));
        let romfs: Vec<String> = nup.part(&t, Part::Romfs).into_iter().map(|f| f.0).collect();
        assert!(romfs.contains(&"Pack/ZsDic.pack.zs".to_string()) && romfs.contains(&"big.bin".to_string()));
        let v = verify(&nup, None, None, &mut |_, _| {}).unwrap();
        assert_eq!(v.bad.len(), 1, "the damaged .zs: {:?}", v.bad);
        assert!(v.bad[0].contains("Wall_Alb") && v.bad[0].contains("game Zstd header"));

        let out = d.join("x");
        let got = extract(&nup, &t, &out, None, &mut |_| {}).unwrap();
        assert_eq!(got.damaged, ["Model/Wall_Alb.bntx.zs"]);
        assert!(std::fs::read(out.join("romfs/big.bin")).unwrap() == crate::testdata::text());
        assert_eq!(
            std::fs::read(out.join("romfs/Pack/ZsDic.pack.zs")).unwrap()[..4],
            ZSTD,
            "a .zs of the game kept zstd"
        );
        assert_eq!(std::fs::read(out.join("romfs/System/Resource/empty.txt")).unwrap().len(), 0);
        // again: everything is there whole, nothing written
        let again = extract(&nup, &t, &out, None, &mut |_| {}).unwrap();
        assert_eq!((again.files, again.damaged.len()), (0, 1));
        // the update's rtld is no module: taken from the base
        assert_eq!(heal(&nup, &t, &out).unwrap(), [("rtld".to_string(), 0)]);
        assert_eq!(std::fs::read(out.join("exefs/rtld")).unwrap(), b"NSO0rtld");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_directory_or_file_with_two_parents_is_refused_at_open() {
        let d = scratch("nup-two-parents");
        let p = d.join("game.nup");
        let good = sample().build();
        let (nt, nd) = (le32(&good, 0x84) as usize, le64(&good, 0x88) as usize);
        let dir = |i: usize| 0xb0 + 0x40 * nt + 24 * i;
        // (a directory with children, and another made to list the same ones too)
        let (a, b) = {
            let with: Vec<usize> = (0..nd).filter(|&i| le32(&good, dir(i) + 12) > 0).collect();
            (with[0], (0..nd).find(|&i| i != with[0]).unwrap())
        };
        let mut bytes = good.clone();
        let range = good[dir(a) + 8..dir(a) + 16].to_vec();
        bytes[dir(b) + 8..dir(b) + 16].copy_from_slice(&range);
        std::fs::write(&p, &bytes).unwrap();
        let e = Nup::open(&p).unwrap_err().to_string();
        assert!(e.contains("child of two directories"), "{e}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_record_larger_than_a_module_is_not_read_whole() {
        let d = scratch("nup-read-cap");
        let p = d.join("game.nup");
        let good = sample().build();
        let (nt, nd, nf) = (le32(&good, 0x84) as usize, le64(&good, 0x88) as usize, le64(&good, 0x90) as usize);
        let i = (0..nf).find(|i| good[0xb0 + 0x40 * nt + 24 * nd + 80 * i + 64] == 1).expect("a packed record");
        let at = 0xb0 + 0x40 * nt + 24 * nd + 80 * i;
        let mut bytes = good.clone();
        bytes[at + 8..at + 16].copy_from_slice(&(crate::nso::MAX_MODULE as u64 + 1).to_le_bytes());
        std::fs::write(&p, &bytes).unwrap();
        let nup = Nup::open(&p).unwrap();
        // (refused before anything is decoded or reserved: said as that, not as a frame gone wrong)
        let e = nup.read(i).unwrap_err().to_string();
        assert!(e.contains("larger than any file the builder reads whole"), "{e}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_record_s_extent_outside_the_package_is_refused_at_open() {
        let d = scratch("nup-extent");
        let p = d.join("game.nup");
        let good = sample().build();
        let (nt, nd, nf) = (le32(&good, 0x84) as usize, le64(&good, 0x88) as usize, le64(&good, 0x90) as usize);
        let mut tried = 0;
        for i in 0..nf {
            let at = 0xb0 + 0x40 * nt + 24 * nd + 80 * i;
            let (size, codec) = (le64(&good, at + 8), good[at + 64]);
            if size == 0 {
                continue; // (no data: no extent)
            }
            let mut cases = vec![(16, u64::MAX - 4), (16, good.len() as u64)];
            if codec == 0 {
                cases.push((8, u64::MAX)); // (a stored file's size is its extent)
            }
            for (field, v) in cases {
                let mut bytes = good.clone();
                bytes[at + field..at + field + 8].copy_from_slice(&v.to_le_bytes());
                std::fs::write(&p, &bytes).unwrap();
                assert!(Nup::open(&p).is_err(), "record {i}: field {field} = {v:#x} opened");
                tried += 1;
            }
        }
        assert!(tried > 6);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_cut_package_is_said_so() {
        let d = scratch("nup-cut");
        let p = d.join("game.nup");
        let bytes = sample().build();
        std::fs::write(&p, &bytes[..bytes.len() - 100]).unwrap();
        assert!(matches!(Nup::open(&p), Err(Error::Player { code: "nup_incomplete", .. })));
        std::fs::write(&p, b"NOPE").unwrap();
        assert!(matches!(Nup::open(&p), Err(Error::Player { code: "not_a_nup", .. })));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn unsafe_paths_are_refused() {
        for bad in ["", "a//b", "../x", "a/./b", "con.txt", "COM1", "a\\b", "x ", "x.", "a:b", "lpt²"] {
            assert!(package_parts(bad).is_err(), "{bad:?}");
        }
        assert_eq!(package_parts("Pack/ZsDic.pack.zs").unwrap(), ["Pack", "ZsDic.pack.zs"]);
        assert!(package_parts("console.txt").is_ok() && package_parts("COM10").is_ok());
    }

    #[test]
    fn a_read_error_in_plain_words() {
        let e = std::io::Error::from_raw_os_error(if cfg!(windows) { 33 } else { 5 });
        let said = unreadable(Path::new("x.nup"), 3 << 20, Some(10 << 20), &e);
        assert_eq!(said.code(), if cfg!(windows) { "nup_locked" } else { "nup_drive_error" });
        assert!(said.to_string().contains("x.nup at 3 of 10 MB"));
    }
}

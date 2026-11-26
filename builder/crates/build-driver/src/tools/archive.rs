//! A tool's archive: a .tar.zst of its own files (prefix `-`), made the same way every time ([`pack`]: sorted, every
//! time and owner fixed, each file 0755 when it is executable else 0644, each relative link inside it a link; the zstd
//! program at `-19 --long=27`: the same files and the same zstd give the same bytes) and unpacked safely ([`unpack`]:
//! nothing written outside its folder, nothing through a link it wrote, nothing half unpacked left). [`install`] puts
//! each tool of a lock at its dest with a stamp beside, from a cache of downloaded archives ([`fetch`]).
//!
//! The tar is GNU tar's format as Python's tarfile wrote it (GNU_FORMAT), byte for byte: the tools' archives made
//! before this reader and writer were Python's.

use super::lock::{EXT, Tool, path_ok};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The zstd program's arguments for an archive (`-T0`: zstd's output is the same with any number of threads).
pub const ZSTD_ARGS: [&str; 4] = ["-19", "--long=27", "-T0", "-q"];
/// Where each tool's stamp is kept, under the folder it was installed into.
pub const STAMPS: &str = ".tools";
const BLOCK: usize = 512;
const RECORD: usize = 20 * BLOCK;

/// The zstd program: `ZSTD` (a developer's or CI's pinned one, builder/tools/lib.sh), else the first on the PATH.
pub fn zstd() -> Result<PathBuf, String> {
    if let Some(z) = crate::env("ZSTD").filter(|z| !z.is_empty()) {
        return Ok(PathBuf::from(z));
    }
    let name = if cfg!(windows) { "zstd.exe" } else { "zstd" };
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
        .ok_or_else(|| "no zstd program (builder/tools/make.sh builds the pinned one: ZSTD)".into())
}

pub use crate::sha256_file;

pub use build_core::fsx::{link_inside as link_ok, through_link};

// ---- the tar format ----

/// A number in a header field of `digits` bytes: octal digits and a NUL, as tarfile's itn (GNU's base-256 past that).
fn itn(n: u64, digits: usize) -> Vec<u8> {
    if n < 8u64.pow((digits - 1) as u32) {
        let mut v = format!("{n:0width$o}", width = digits - 1).into_bytes();
        v.push(0);
        v
    } else {
        let mut v = vec![0u8; digits];
        v[0] = 0x80;
        for (i, b) in n.to_be_bytes().iter().rev().enumerate() {
            v[digits - 1 - i] = *b;
        }
        v
    }
}

/// A string in a header field of `len` bytes, cut or padded with NULs (tarfile's stn).
fn stn(s: &[u8], len: usize) -> Vec<u8> {
    let mut v = s[..s.len().min(len)].to_vec();
    v.resize(len, 0);
    v
}

/// One header block (tarfile's _create_header, GNU's magic): no device fields but for devices.
fn header(name: &[u8], mode: u32, size: u64, kind: u8, link: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(BLOCK);
    b.extend(stn(name, 100));
    b.extend(itn(u64::from(mode & 0o7777), 8));
    b.extend(itn(0, 8));
    b.extend(itn(0, 8));
    b.extend(itn(size, 12));
    b.extend(itn(0, 12));
    b.extend(b"        ");
    b.push(kind);
    b.extend(stn(link, 100));
    b.extend(b"ustar  \0");
    b.extend(stn(b"", 32));
    b.extend(stn(b"", 32));
    b.extend(stn(b"", 8));
    b.extend(stn(b"", 8));
    b.extend(stn(b"", 155));
    b.resize(BLOCK, 0);
    let sum: u32 = b.iter().map(|&c| u32::from(c)).sum();
    b[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    b
}

fn padded(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    v.resize(data.len().div_ceil(BLOCK) * BLOCK, 0);
    v
}

/// An entry's headers: GNU's long-name (`L`) and long-link (`K`) entries first where a name passes 100 bytes.
fn headers(name: &str, mode: u32, size: u64, kind: u8, link: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (long, k) in [(link, b'K'), (name, b'L')] {
        if long.len() > 100 {
            let mut v = long.as_bytes().to_vec();
            v.push(0);
            out.extend(header(b"././@LongLink", 0, v.len() as u64, k, b""));
            out.extend(padded(&v));
        }
    }
    out.extend(header(name.as_bytes(), mode, size, kind, link.as_bytes()));
    out
}

/// What a folder holds, for [`pack`]: (relative path, full path), every folder, file and link, sorted by path (a link to
/// a folder is a link, not walked).
pub(crate) fn entries(folder: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        let read = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in read {
            let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
            let name = e.file_name().into_string().map_err(|n| format!("{}: {n:?} is not UTF-8", dir.display()))?;
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let path = e.path();
            let real_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            out.push((r.clone(), path.clone()));
            if real_dir {
                walk(&path, &r, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(folder, "", &mut out)?;
    out.sort();
    Ok(out)
}

#[cfg(unix)]
pub(crate) fn executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
pub(crate) fn executable(_: &std::fs::Metadata) -> bool {
    true // (as Python's os.access(X_OK) on Windows: any file)
}

/// `folder` as a tool's archive at `out`, its files at the top (prefix `-`). Returns (SHA-256, size).
pub fn pack(folder: &Path, out: &Path) -> Result<(String, u64), String> {
    let folder = std::path::absolute(folder).map_err(|e| e.to_string())?;
    let all = entries(&folder)?;
    if !all.iter().any(|(_, p)| std::fs::symlink_metadata(p).is_ok_and(|m| !m.is_dir())) {
        return Err(format!("{} is empty", folder.display()));
    }
    let tar_path = PathBuf::from(format!("{}.tar.part", out.display()));
    let written = (|| -> Result<(), String> {
        let f = std::fs::File::create(&tar_path).map_err(|e| format!("{}: {e}", tar_path.display()))?;
        let mut t = std::io::BufWriter::new(f);
        let at = std::cell::Cell::new(0usize);
        let put = |t: &mut std::io::BufWriter<std::fs::File>, b: &[u8]| -> Result<(), String> {
            at.set(at.get() + b.len());
            t.write_all(b).map_err(|e| format!("{}: {e}", tar_path.display()))
        };
        for (rel, full) in &all {
            let m = std::fs::symlink_metadata(full).map_err(|e| format!("{}: {e}", full.display()))?;
            if m.file_type().is_symlink() {
                let target = std::fs::read_link(full).map_err(|e| format!("{}: {e}", full.display()))?;
                let target = target.to_str().ok_or_else(|| format!("{rel}: a link that is not UTF-8"))?.to_string();
                if !link_ok(rel, &target) {
                    return Err(format!("{}: {rel} links outside the folder ({target})", folder.display()));
                }
                put(&mut t, &headers(rel, 0o777, 0, b'2', &target))?;
            } else if m.is_dir() {
                put(&mut t, &headers(&format!("{rel}/"), 0o755, 0, b'5', ""))?;
            } else if m.is_file() {
                let mode = if executable(&m) { 0o755 } else { 0o644 };
                put(&mut t, &headers(rel, mode, m.len(), b'0', ""))?;
                let mut src = std::fs::File::open(full).map_err(|e| format!("{}: {e}", full.display()))?;
                let mut buf = vec![0u8; 1 << 20];
                let mut left = m.len();
                while left > 0 {
                    let n = src.read(&mut buf).map_err(|e| format!("{}: {e}", full.display()))?;
                    if n == 0 {
                        return Err(format!("{}: changed while packed", full.display()));
                    }
                    let n = n.min(left as usize);
                    put(&mut t, &buf[..n])?;
                    left -= n as u64;
                }
                let rest = (BLOCK - (m.len() as usize % BLOCK)) % BLOCK;
                put(&mut t, &vec![0u8; rest])?;
            } else {
                return Err(format!("{}: {rel} is not a file", folder.display()));
            }
        }
        put(&mut t, &[0u8; 2 * BLOCK])?;
        let rest = (RECORD - at.get() % RECORD) % RECORD;
        put(&mut t, &vec![0u8; rest])?;
        t.flush().map_err(|e| format!("{}: {e}", tar_path.display()))
    })()
    .and_then(|()| {
        let s = build_core::child::status(
            build_core::child::command(zstd()?).args(ZSTD_ARGS).arg("-f").arg("-o").arg(out).arg(&tar_path),
        )
        .map_err(|e| format!("zstd did not start: {e}"))?;
        if s.success() { Ok(()) } else { Err(format!("zstd failed ({s})")) }
    });
    let _ = std::fs::remove_file(&tar_path);
    written?;
    let (sha, size) = sha256_file(out)?;
    println!("{}: {} entries, {size} bytes, sha256 {sha} (prefix -)", out.display(), all.len());
    Ok((sha, size))
}

/// One entry of a tar stream, its data unread ([`Reader::data`], [`Reader::skip`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its path as the archive names it (GNU's long name or pax's `path` taken in).
    pub name: String,
    /// Its type flag: `b'0'` a file (a NUL flag read as one), `b'5'` a folder, `b'2'` a symbolic link, ...
    pub kind: u8,
    /// Its mode bits.
    pub mode: u32,
    /// Its data's size.
    pub size: u64,
    /// A link's target.
    pub link: String,
}

fn field_str(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn field_num(b: &[u8]) -> Result<u64, String> {
    if b[0] & 0x80 != 0 {
        return Ok(b[1..].iter().fold(0u64, |n, &c| (n << 8) | u64::from(c)));
    }
    let s = field_str(b);
    let s = s.trim_matches(|c: char| c == ' ' || c == '\0');
    if s.is_empty() { Ok(0) } else { u64::from_str_radix(s, 8).map_err(|_| format!("a bad number in a header: {s:?}")) }
}

/// The records of a pax header's data (`<its length> <key>=<value>\n` each), in order. A length that does not cover
/// its own digits and space, or runs past the data, is refused, never read past.
fn pax_records(data: &[u8]) -> Result<Vec<(String, String)>, String> {
    let text = String::from_utf8_lossy(data).into_owned();
    let mut rest = text.as_str();
    let mut out = Vec::new();
    while let Some((len, _)) = rest.split_once(' ') {
        let n: usize = len.parse().map_err(|_| "a bad pax header".to_string())?;
        let rec = rest.get(..n).filter(|_| n > len.len() + 1).ok_or("a bad pax header")?;
        let kv = rec.get(len.len() + 1..).ok_or("a bad pax header")?.trim_end_matches('\n');
        if let Some((k, v)) = kv.split_once('=') {
            out.push((k.to_string(), v.to_string()));
        }
        rest = &rest[n..];
    }
    Ok(out)
}

/// A tar stream's entries: GNU's long names and links and pax's `path` and `linkpath` taken in; pax's global
/// records (git archive's `comment`, the commit) kept in [`Reader::globals`].
pub struct Reader<R: Read> {
    /// The stream, at the next header or at the current entry's data.
    pub r: R,
    /// The pax global header's records read so far (`g` entries), in order.
    pub globals: Vec<(String, String)>,
}

impl<R: Read> Reader<R> {
    /// A reader of the tar stream `r`.
    pub fn new(r: R) -> Reader<R> {
        Reader { r, globals: Vec::new() }
    }

    /// The current entry's data (`size` bytes: [`Entry::size`]), read whole, and its padding skipped.
    pub fn data(&mut self, size: u64) -> Result<Vec<u8>, String> {
        let mut v = vec![0u8; usize::try_from(size).map_err(|_| "a header's size".to_string())?];
        self.r.read_exact(&mut v).map_err(|e| e.to_string())?;
        self.skip_pad(size)?;
        Ok(v)
    }

    /// The current entry's data (`size` bytes) copied into `out`, and its padding skipped.
    pub fn copy_to(&mut self, size: u64, out: &mut impl Write) -> Result<(), String> {
        let got = std::io::copy(&mut (&mut self.r).take(size), out).map_err(|e| e.to_string())?;
        if got != size {
            return Err("cut short".into());
        }
        self.skip_pad(size)
    }

    /// The current entry's data (`size` bytes) passed over, with its padding.
    pub fn skip(&mut self, size: u64) -> Result<(), String> {
        self.copy_to(size, &mut std::io::sink())
    }

    fn skip_pad(&mut self, size: u64) -> Result<(), String> {
        let pad = (BLOCK - (size as usize % BLOCK)) % BLOCK;
        let mut b = vec![0u8; pad];
        self.r.read_exact(&mut b).map_err(|e| e.to_string())
    }

    /// The next entry, or none at the end (the first block of zeros).
    #[allow(clippy::should_implement_trait, reason = "fallible: not an Iterator's next")]
    pub fn next(&mut self) -> Result<Option<Entry>, String> {
        let (mut long_name, mut long_link) = (None, None);
        loop {
            let mut h = [0u8; BLOCK];
            match self.r.read_exact(&mut h) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(e) => return Err(e.to_string()),
            }
            if h.iter().all(|&c| c == 0) {
                return Ok(None);
            }
            let sum: u64 =
                h.iter().enumerate().map(|(i, &c)| if (148..156).contains(&i) { 32 } else { u64::from(c) }).sum();
            if field_num(&h[148..156])? != sum {
                return Err("a header whose checksum is wrong".into());
            }
            let size = field_num(&h[124..136])?;
            let kind = h[156];
            match kind {
                b'L' => long_name = Some(field_str(&self.data(size)?)),
                b'K' => long_link = Some(field_str(&self.data(size)?)),
                b'x' => {
                    for (k, v) in pax_records(&self.data(size)?)? {
                        match k.as_str() {
                            "path" => long_name = Some(v),
                            "linkpath" => long_link = Some(v),
                            _ => {}
                        }
                    }
                }
                b'g' => {
                    let records = pax_records(&self.data(size)?)?;
                    self.globals.extend(records);
                }
                _ => {
                    let mut name = field_str(&h[0..100]);
                    let prefix = field_str(&h[345..500]);
                    if &h[257..263] == b"ustar\0" && !prefix.is_empty() {
                        name = format!("{prefix}/{name}");
                    }
                    let kind = if kind == 0 { b'0' } else { kind };
                    return Ok(Some(Entry {
                        name: long_name.unwrap_or(name),
                        kind,
                        mode: field_num(&h[100..108])? as u32,
                        size,
                        link: long_link.unwrap_or_else(|| field_str(&h[157..257])),
                    }));
                }
            }
        }
    }
}

/// The archive's members under `prefix` (`-`: all) into `dest`, which must not be there yet, through `dest.part`:
/// nothing outside `dest` is written and nothing half unpacked is left. A file keeps whether it is executable, and a
/// link stays a link, relative and inside `dest`. Returns how many files and links were unpacked.
pub fn unpack(archive: &Path, prefix: &str, dest: &Path) -> Result<u64, String> {
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err(format!("{} is there already", dest.display()));
    }
    let tmp = PathBuf::from(format!("{}.part", dest.display()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let lead = if prefix == "-" { String::new() } else { format!("{}/", prefix.trim_end_matches('/')) };
    let done = unpack_into(archive, &lead, &tmp)
        .and_then(|n| if n == 0 { Err(format!("{} has nothing under {prefix}", archive.display())) } else { Ok(n) });
    match done {
        Ok(n) => {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            std::fs::rename(&tmp, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

fn unpack_into(archive: &Path, lead: &str, tmp: &Path) -> Result<u64, String> {
    let at = |e: String| format!("{}: {e}", archive.display());
    let mut z = build_core::child::spawn(
        build_core::child::command(zstd()?).args(["-dc", "--long=27"]).arg(archive).stdout(Stdio::piped()),
    )
    .map_err(|e| format!("zstd did not start: {e}"))?;
    let out = z.stdout.take().ok_or("zstd's output")?;
    let mut t = Reader::new(std::io::BufReader::with_capacity(1 << 20, out));
    let read = (|| -> Result<u64, String> {
        let mut n = 0;
        while let Some(m) = t.next().map_err(at)? {
            let take = m.name.starts_with(lead) && m.kind != b'5';
            if !take {
                if matches!(m.kind, b'0' | b'7') {
                    std::io::copy(&mut (&mut t.r).take(m.size), &mut std::io::sink()).map_err(|e| at(e.to_string()))?;
                    t.skip_pad(m.size).map_err(at)?;
                }
                continue;
            }
            let rel = &m.name[lead.len()..];
            if !path_ok(rel) {
                return Err(at(format!("{} leaves the folder", m.name)));
            }
            if let Some(link) = through_link(tmp, rel) {
                return Err(at(format!("{} goes through the link {link}", m.name)));
            }
            let out = tmp.join(rel);
            if std::fs::symlink_metadata(&out).is_ok() {
                return Err(at(format!("{} is there twice", m.name)));
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            match m.kind {
                b'2' => {
                    if !link_ok(rel, &m.link) {
                        return Err(at(format!("{} links outside the folder ({})", m.name, m.link)));
                    }
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&m.link, &out).map_err(|e| format!("{}: {e}", out.display()))?;
                    #[cfg(not(unix))]
                    return Err(at(format!("{}: a symbolic link, on a system that unpacks none", m.name)));
                }
                b'0' | b'7' => {
                    let mut f = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
                    let got = std::io::copy(&mut (&mut t.r).take(m.size), &mut f).map_err(|e| at(e.to_string()))?;
                    if got != m.size {
                        return Err(at("cut short".into()));
                    }
                    t.skip_pad(m.size).map_err(at)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = if m.mode & 0o111 != 0 { 0o755 } else { 0o644 };
                        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))
                            .map_err(|e| format!("{}: {e}", out.display()))?;
                    }
                }
                _ => return Err(at(format!("{} is neither a file, a folder nor a link", m.name))),
            }
            n += 1;
        }
        Ok(n)
    })();
    // (read to zstd's end: closed early, the pipe would kill zstd (SIGPIPE) while it writes the tar's last records,
    // and a whole archive would be said unreadable)
    let _ = std::io::copy(&mut t.r, &mut std::io::sink());
    drop(t);
    let status = z.wait().map_err(|e| e.to_string())?;
    let n = read?;
    if !status.success() {
        return Err(at("zstd could not read it".into()));
    }
    Ok(n)
}

// ---- installing ----

/// The SHA-256 of the archive the tool in `folder` was unpacked from (`folder/.tools/<name>`), if any.
pub fn stamp_of(folder: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(folder.join(STAMPS).join(name)).ok()?.split_whitespace().next().map(str::to_string)
}

fn write_stamp(folder: &Path, t: &Tool) -> Result<(), String> {
    let d = folder.join(STAMPS);
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    std::fs::write(d.join(&t.name), format!("{} {} {}\n", t.sha256, t.prefix, t.dest))
        .map_err(|e| format!("{}: {e}", d.display()))
}

/// Gets a URL's bytes into a file: the network's ([`super::store::download`]), or a test's.
pub type Getter<'a> = &'a dyn Fn(&str, &Path, u64) -> Result<(), String>;

/// The tool's archive in `cache` as `<sha256>.tar.zst` (one cache for every version), from its first URL that gives
/// the right bytes (no more than its size; its SHA-256), through `<file>.part`.
pub fn fetch(t: &Tool, cache: &Path, get: Getter<'_>) -> Result<PathBuf, String> {
    let path = cache.join(format!("{}{EXT}", t.sha256));
    if path.is_file() && sha256_file(&path).is_ok_and(|(s, _)| s == t.sha256) {
        return Ok(path);
    }
    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let part = PathBuf::from(format!("{}.part", path.display()));
    let mut error = "no URL".to_string();
    for url in &t.urls {
        let _ = std::fs::remove_file(&part);
        let got = get(url, &part, t.size).and_then(|()| {
            let (sha, size) = sha256_file(&part)?;
            if size > t.size {
                Err(format!("{url}: larger than the {} bytes pinned", t.size))
            } else if sha != t.sha256 {
                Err(format!("{url}: not the file pinned (SHA-256 {sha}, want {}): not used", t.sha256))
            } else {
                Ok(())
            }
        });
        match got {
            Ok(()) => {
                std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
                return Ok(path);
            }
            Err(e) => error = e,
        }
    }
    let _ = std::fs::remove_file(&part);
    Err(format!("{}: {error}", t.name))
}

/// Each tool's archive unpacked at its dest under `folder`, its stamp beside (`folder/.tools/<name>`). A tool there
/// with another stamp (the lock changed), or none, is unpacked again.
pub fn install(folder: &Path, tools: &[&Tool], cache: &Path, get: Getter<'_>) -> Result<(), String> {
    for t in tools {
        let dest = folder.join(&t.dest);
        if dest.is_dir() {
            if stamp_of(folder, &t.name).as_deref() == Some(t.sha256.as_str()) {
                println!("{:<12} there already", t.name);
                continue;
            }
            println!("{:<12} {}: not tools.lock's archive, unpacked again", t.name, t.version);
            std::fs::remove_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        let n = unpack(&fetch(t, cache, get)?, &t.prefix, &dest)?;
        write_stamp(folder, t)?;
        println!("{:<12} {}: {n} files into {}", t.name, t.version, t.dest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::lock::parse;
    use super::*;

    fn have_zstd() -> bool {
        zstd().is_ok()
    }

    fn archive(d: &Path, name: &str, files: &[(&str, &str)]) -> (PathBuf, String, u64) {
        let src = d.join(format!("src-{name}"));
        for (rel, text) in files {
            std::fs::create_dir_all(src.join(rel).parent().unwrap()).unwrap();
            std::fs::write(src.join(rel), text).unwrap();
        }
        let out = d.join(format!("{name}{EXT}"));
        let (sha, size) = pack(&src, &out).unwrap();
        (out, sha, size)
    }

    fn line(name: &str, version: &str, system: &str, sha: &str, size: u64) -> Tool {
        parse(&super::super::lock::tests::line(name, version, system, sha, size)).unwrap().remove(0)
    }

    #[test]
    fn a_bad_pax_record_is_refused_not_a_panic_or_a_loop() {
        for rec in [&b"1 x\n"[..], b"0 ", b"99 path=a\n", b"x path=a\n"] {
            let mut tar = header(b"././@PaxHeader", 0o644, rec.len() as u64, b'x', b"");
            tar.extend_from_slice(rec);
            tar.resize(tar.len().div_ceil(BLOCK) * BLOCK, 0);
            tar.extend(header(b"a", 0o644, 0, b'0', b""));
            let mut r = Reader::new(std::io::Cursor::new(tar));
            assert!(r.next().is_err(), "{:?}", String::from_utf8_lossy(rec));
        }
        let rec = b"11 path=ab\n";
        let mut tar = header(b"././@PaxHeader", 0o644, rec.len() as u64, b'x', b"");
        tar.extend_from_slice(rec);
        tar.resize(tar.len().div_ceil(BLOCK) * BLOCK, 0);
        tar.extend(header(b"a", 0o644, 0, b'0', b""));
        assert_eq!(Reader::new(std::io::Cursor::new(tar)).next().unwrap().unwrap().name, "ab");
    }

    #[test]
    fn a_header_is_python_s_tarfile_s() {
        // (tarfile.TarInfo("a/b.txt"), size 5, mode 0o644, GNU_FORMAT: its 512 bytes' checksum and fields)
        let h = header(b"a/b.txt", 0o644, 5, b'0', b"");
        assert_eq!(&h[100..108], b"0000644\0");
        assert_eq!(&h[124..136], b"00000000005\0");
        assert_eq!(&h[257..265], b"ustar  \0");
        assert!(h[329..345].iter().all(|&c| c == 0), "no device fields");
        let sum: u32 =
            h.iter().enumerate().map(|(i, &c)| if (148..156).contains(&i) { 32 } else { u32::from(c) }).sum();
        assert_eq!(&h[148..156], format!("{sum:06o}\0 ").as_bytes());
        let long = headers(&"x".repeat(120), 0o644, 0, b'0', "");
        assert_eq!((long.len(), long[156]), (3 * BLOCK, b'L'), "a long name: an L header, its name, the header");
        assert_eq!(itn(8u64.pow(11), 12)[0], 0x80, "base-256 past 8 GB");
    }

    #[test]
    fn fetch_tries_the_next_url() {
        if !have_zstd() {
            return;
        }
        let d = crate::scratch("tools-fetch");
        let (path, sha, size) = archive(&d, "t", &[("x.txt", "x")]);
        let data = std::fs::read(&path).unwrap();
        let t = line("t", "1", "windows-x64", &sha, size);
        let asked = std::cell::RefCell::new(Vec::new());
        let get = |url: &str, to: &Path, _: u64| -> Result<(), String> {
            asked.borrow_mut().push(url.to_string());
            std::fs::write(to, if url.contains("git-cdn") { b"changed".to_vec() } else { data.clone() })
                .map_err(|e| e.to_string())
        };
        let got = fetch(&t, &d.join("cache"), &get).unwrap();
        assert_eq!((std::fs::read(&got).unwrap(), asked.borrow().len()), (data, 2));
        let again = |_: &str, _: &Path, _: u64| -> Result<(), String> { panic!("downloaded again") };
        assert_eq!(fetch(&t, &d.join("cache"), &again).unwrap(), got);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn install_stamps_and_unpacks_a_changed_line_again() {
        if !have_zstd() {
            return;
        }
        let d = crate::scratch("tools-install");
        let (a, sha, size) = archive(&d, "a", &[("old.txt", "1")]);
        let old = line("t", "1", "linux-x64", &sha, size);
        let from =
            |p: PathBuf| move |_: &str, to: &Path, _: u64| std::fs::copy(&p, to).map(|_| ()).map_err(|e| e.to_string());
        install(&d.join("b"), &[&old], &d.join("cache"), &from(a.clone())).unwrap();
        assert_eq!(stamp_of(&d.join("b"), "t"), Some(sha));
        let never = |_: &str, _: &Path, _: u64| -> Result<(), String> { panic!("again") };
        install(&d.join("b"), &[&old], &d.join("cache"), &never).unwrap();
        let (b, sha2, size2) = archive(&d, "b", &[("new.txt", "2")]);
        let new = line("t", "2", "linux-x64", &sha2, size2);
        install(&d.join("b"), &[&new], &d.join("cache"), &from(b)).unwrap();
        let names: Vec<String> = std::fs::read_dir(d.join("b/tools/t"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["new.txt"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn pack_is_deterministic_with_modes_and_links() {
        use std::os::unix::fs::PermissionsExt;
        if !have_zstd() {
            return;
        }
        let d = crate::scratch("tools-pack");
        let src = d.join("llvm");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/clang"), "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(src.join("bin/clang"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(src.join("LICENSE.TXT"), "license").unwrap();
        std::os::unix::fs::symlink("bin", src.join("b")).unwrap();
        let first = pack(&src, &d.join("1.tar.zst")).unwrap();
        let f = std::fs::File::options().write(true).open(src.join("LICENSE.TXT")).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)).unwrap();
        assert_eq!(pack(&src, &d.join("2.tar.zst")).unwrap(), first, "times are not in it");
        unpack(&d.join("1.tar.zst"), "-", &d.join("out")).unwrap();
        let mode = |p: &str| std::fs::metadata(d.join("out").join(p)).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode("bin/clang"), mode("LICENSE.TXT")), (0o755, 0o644));
        assert_eq!(std::fs::read_link(d.join("out/b")).unwrap(), Path::new("bin"));
        for target in ["/usr/bin/clang", "../../outside"] {
            std::os::unix::fs::symlink(target, src.join("bad")).unwrap();
            assert!(pack(&src, &d.join("bad.tar.zst")).is_err(), "{target}");
            std::fs::remove_file(src.join("bad")).unwrap();
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A tar of `(name, kind, data)` (kind `0` a file, `2` a link to `data`), as another program might write it.
    #[cfg(unix)]
    fn raw_archive(path: &Path, entries: &[(&str, u8, &str)]) {
        let mut t = Vec::new();
        for (name, kind, data) in entries {
            if *kind == b'2' {
                t.extend(headers(name, 0o777, 0, b'2', data));
            } else {
                t.extend(headers(name, 0o644, data.len() as u64, b'0', ""));
                t.extend(padded(data.as_bytes()));
            }
        }
        t.extend([0u8; 2 * BLOCK]);
        let tar = PathBuf::from(format!("{}.tar", path.display()));
        std::fs::write(&tar, t).unwrap();
        assert!(
            std::process::Command::new(zstd().unwrap())
                .args(["-q", "-f", "-o"])
                .arg(path)
                .arg(&tar)
                .status()
                .unwrap()
                .success()
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_archive_s_links_never_lead_a_file_out() {
        if !have_zstd() {
            return;
        }
        let d = crate::scratch("tools-links");
        // (a link out, then a file through it; a link to a link in a folder, then a file through the nested link; a
        // name with ..; a link whose .. climbs past the folder)
        let hostile: [&[(&str, u8, &str)]; 4] = [
            &[("a", b'2', "."), ("a/escaped", b'0', "x")],
            &[("d/up", b'2', ".."), ("d/up/up2", b'2', ".."), ("d/up/up2/escaped", b'0', "x")],
            &[("../escaped", b'0', "x")],
            &[("d/out", b'2', "../../escaped")],
        ];
        for (n, entries) in hostile.iter().enumerate() {
            let path = d.join(format!("t{n}.tar.zst"));
            raw_archive(&path, entries);
            assert!(unpack(&path, "-", &d.join(format!("tool{n}"))).is_err(), "{entries:?}");
            assert!(!d.join(format!("tool{n}.part")).exists(), "nothing half unpacked is left");
        }
        assert!(!d.join("escaped").exists() && !d.parent().unwrap().join("escaped").exists(), "a file landed outside");
        let fine = d.join("f.tar.zst");
        raw_archive(&fine, &[("bin/x", b'0', "x"), ("lib", b'2', "bin")]);
        assert_eq!(unpack(&fine, "-", &d.join("fine")).unwrap(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}

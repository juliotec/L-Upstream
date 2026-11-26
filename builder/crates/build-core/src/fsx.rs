//! File operations a build must make safe against a stopped run, a full disk, a second program in the folder.
//!
//! A file the build writes is whole or not there: it is written beside its place and renamed over it
//! ([`write_atomic`]), so a run stopped half way never leaves half a file that the next one takes for done (the lift's
//! "cut short" checks exist because of that). A file that is the same as before is not touched ([`write_if_changed`]),
//! so its date stays and nothing that depends on it is rebuilt.

use crate::Result;
use crate::error::IoContext;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// `path`'s parent, made if it is missing.
fn parent_made(path: &Path) -> Result<()> {
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => fs::create_dir_all(dir).at(dir),
        None => Ok(()),
    }
}

/// The temporary name `path` is written to before it takes its place: beside it, so the rename stays on one drive;
/// this process's and this call's own, so two writers (two builds, two threads) never share one.
pub fn temporary(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}-{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Whether `a` and `b` are one file (the same file id: a hard link of it, or the same path by another spelling).
pub fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(a), fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // (std tells no file id on Windows: the same path, however spelled)
        matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
    }
}

/// `from` renamed to `to` (over it, if it is there). On Windows an antivirus or the search indexer opening a file
/// just written refuses its rename for a moment (access denied, a sharing violation): tried again for about two
/// seconds, backing off, before the error is said.
pub fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut wait = std::time::Duration::from_millis(10);
    let mut tries = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if cfg!(windows) && tries < 8 && transient(&e) => {
                std::thread::sleep(wait);
                wait *= 2;
                tries += 1;
            }
            done => return done,
        }
    }
}

/// An error a Windows rename gives while another program has the file open for a moment: ERROR_ACCESS_DENIED (5),
/// ERROR_SHARING_VIOLATION (32), ERROR_LOCK_VIOLATION (33).
fn transient(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// `bytes` written to `path` whole: to a temporary file beside it, flushed to the disk, then renamed over it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    parent_made(path)?;
    let tmp = temporary(path);
    let written = (|| {
        // (create_new: a name of this call's own, never a link someone left at it followed)
        let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(crate::Error::io(path, e));
    }
    rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        crate::Error::io(path, e)
    })
}

/// `bytes` written to `path` whole, as [`write_atomic`] but not flushed to the disk first: for many small files a
/// cache can lose (the object store's manifests), where a flush each would cost seconds.
pub fn write_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    parent_made(path)?;
    let tmp = temporary(path);
    let written = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).and_then(|mut f| f.write_all(bytes));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(crate::Error::io(path, e));
    }
    rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        crate::Error::io(path, e)
    })
}

/// `bytes` written to `path` only if it holds something else (or nothing): whether it was written.
pub fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    match fs::read(path) {
        Ok(old) if old == bytes => Ok(false),
        _ => write_atomic(path, bytes).map(|()| true),
    }
}

/// A folder and everything under it removed; nothing to do if it is not there.
pub fn remove_tree(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(crate::Error::io(path, e)),
        _ => Ok(()),
    }
}

/// A file removed; nothing to do if it is not there.
pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(crate::Error::io(path, e)),
        _ => Ok(()),
    }
}

/// `path` removed now, or later: whether it is gone now (nothing there: gone). A file a running program holds
/// (Windows: an old exe) stays; Windows is asked to remove it when the PC restarts, which it does only for an
/// administrator, so for a player it is the next build that removes it (the game folder's sweep of `.old` files).
pub fn remove_or_later(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => {
            let _ = crate::sys::delete_at_restart(path);
            false
        }
    }
}

/// How a file got to its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    /// A hard link to the source (the same data, no copy).
    Linked,
    /// A copy.
    Copied,
}

/// `from` hard-linked at `to`, or copied where a link is refused (another drive, a file system without links, a
/// sandbox): the romfs in a game folder is linked to the unpacked one, not copied (26 GB). Made beside `to` and
/// renamed over it, so `to` is never removed first: when it is `from` itself (or a link of it) nothing is done, and
/// the source's data is never lost however the two paths are spelled.
pub fn link_or_copy(from: &Path, to: &Path) -> Result<Placed> {
    parent_made(to)?;
    if same_file(from, to) {
        return Ok(Placed::Linked);
    }
    let tmp = temporary(to);
    let how = match fs::hard_link(from, &tmp) {
        Ok(()) => Placed::Linked,
        Err(_) => {
            if let Err(e) = fs::copy(from, &tmp) {
                let _ = fs::remove_file(&tmp);
                return Err(crate::Error::io(to, e));
            }
            Placed::Copied
        }
    };
    rename(&tmp, to).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        crate::Error::io(to, e)
    })?;
    Ok(how)
}

/// A path a list, an archive or a note may name inside its folder: relative, `/`-separated, each part a name (no
/// empty part, no `.` or `..`), with no drive, stream or `\` in it. The one rule for the carried sources' members,
/// the tools' archives, tools.lock's folders and the game folder's installed note (tools.lock's names also keep to
/// a few characters: `tools::lock::path_ok`).
pub fn relative_ok(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// A symbolic link at `rel` (inside its folder, [`relative_ok`]) to `target` that stays inside the folder: relative,
/// its `..` all first and fewer than `rel`'s folders, the rest names. The stricter of the two rules the unpackers
/// had: a `..` in the middle (`a/../b`) is refused too.
pub fn link_inside(rel: &str, target: &str) -> bool {
    let parts: Vec<&str> = target.split('/').collect();
    let ups = parts.iter().take_while(|p| **p == "..").count();
    if target.is_empty() || target.starts_with('/') || target.contains('\\') || target.contains(':') {
        return false;
    }
    if parts[ups..].iter().any(|p| p.is_empty() || *p == "..") {
        return false;
    }
    ups < rel.split('/').count()
}

/// The first folder of `rel` under `dir` that is a symbolic link already there: an unpacker writes nothing whose path
/// goes through a link (`a/b -> ..`, then `a/b/c -> ..`, would escape the folder).
pub fn through_link(dir: &Path, rel: &str) -> Option<String> {
    let mut at = dir.to_path_buf();
    let mut so_far = Vec::new();
    for part in rel.split('/') {
        at.push(part);
        so_far.push(part);
        if fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Some(so_far.join("/"));
        }
    }
    None
}

/// Every file under `dir`, relative to it, sorted (a stable order: what the caches and the lists are made from).
pub fn files_under(dir: &Path) -> Result<Vec<PathBuf>> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(dir).at(dir)? {
            let entry = entry.at(dir)?;
            let path = entry.path();
            let kind = entry.file_type().at(&path)?;
            if kind.is_dir() {
                walk(base, &path, out)?;
            } else {
                out.push(path.strip_prefix(base).unwrap_or(&path).to_path_buf());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// A tree of files placed under `to` as it is under `from`, each linked where it can be, else copied.
pub fn link_tree(from: &Path, to: &Path) -> Result<Placed> {
    let mut how = Placed::Linked;
    for rel in files_under(from)? {
        if link_or_copy(&from.join(&rel), &to.join(&rel))? == Placed::Copied {
            how = Placed::Copied;
        }
    }
    Ok(how)
}

/// A tree of files copied under `to` as it is under `from`.
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for rel in files_under(from)? {
        let dst = to.join(&rel);
        parent_made(&dst)?;
        fs::copy(from.join(&rel), &dst).at(&dst)?;
    }
    Ok(())
}

/// How many names (hard links) the file `p` has, at least 1; links are not followed. 1 when it can't be read.
pub fn link_count(p: &Path, m: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        let _ = p;
        std::os::unix::fs::MetadataExt::nlink(m).max(1)
    }
    #[cfg(windows)]
    #[allow(unsafe_code)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            GetFileInformationByHandle,
        };
        let _ = m;
        // (no access asked: reading the information needs none, and a file another program has open still opens)
        let Ok(f) = fs::OpenOptions::new()
            .access_mode(0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(p)
        else {
            return 1;
        };
        // SAFETY: BY_HANDLE_FILE_INFORMATION is plain data, valid all zero.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is the open file's, and info is a BY_HANDLE_FILE_INFORMATION to fill.
        if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) } == 0 {
            return 1;
        }
        u64::from(info.nNumberOfLinks).max(1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (p, m);
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("build-core-fsx-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_atomic_write_leaves_no_temporary_and_an_unchanged_file_is_not_written() {
        let d = scratch("atomic");
        let p = d.join("a/b.txt");
        write_atomic(&p, b"one").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"one");
        assert!(!write_if_changed(&p, b"one").unwrap());
        assert!(write_if_changed(&p, b"two").unwrap());
        assert_eq!(files_under(&d).unwrap(), [PathBuf::from("a/b.txt")]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn a_link_at_the_temporary_s_name_is_not_written_through() {
        let d = std::env::temp_dir().join(format!("build-core-fsx-tmp-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let victim = d.join("victim");
        fs::write(&victim, b"theirs").unwrap();
        // (the next temporary's name, guessed and made a link to another file)
        let at = d.join("f");
        let next = temporary(&at);
        let name = next.file_name().unwrap().to_string_lossy().into_owned();
        let n: u64 = name.trim_end_matches(".tmp").rsplit('-').next().unwrap().parse().unwrap();
        // (the next few names: other tests take some of the count meanwhile)
        for k in n + 1..n + 64 {
            std::os::unix::fs::symlink(&victim, d.join(format!("f.{}-{k}.tmp", std::process::id()))).unwrap();
        }
        let _ = write_atomic(&at, b"ours");
        let _ = write_replace(&at, b"ours");
        assert_eq!(fs::read(&victim).unwrap(), b"theirs", "written through a link");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn two_writers_never_share_a_temporary() {
        let p = Path::new("/x/runtime.dll");
        assert_ne!(temporary(p), temporary(p));
        assert_eq!(temporary(p).parent(), p.parent(), "beside its place: one drive");
    }

    #[test]
    fn a_link_over_its_own_source_keeps_the_data() {
        let d = scratch("self");
        let a = d.join("a");
        write_atomic(&a, b"data").unwrap();
        assert_eq!(link_or_copy(&a, &a).unwrap(), Placed::Linked);
        assert_eq!(fs::read(&a).unwrap(), b"data", "linking a file over itself lost it");
        let spelled = d.join("x/../a");
        fs::create_dir_all(d.join("x")).unwrap();
        link_or_copy(&a, &spelled).unwrap();
        assert_eq!(fs::read(&a).unwrap(), b"data");
        let b = d.join("b");
        link_or_copy(&a, &b).unwrap();
        link_or_copy(&a, &b).unwrap(); // (already a link of it)
        assert_eq!(fs::read(&b).unwrap(), b"data");
        write_atomic(&d.join("c"), b"other").unwrap();
        link_or_copy(&d.join("c"), &b).unwrap(); // (replaced whole)
        assert_eq!((fs::read(&b).unwrap(), fs::read(&a).unwrap()), (b"other".to_vec(), b"data".to_vec()));
        assert_eq!(files_under(&d).unwrap().len(), 3, "no temporary left");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_tree_is_linked_or_copied_whole() {
        let d = scratch("tree");
        write_atomic(&d.join("src/x/1"), b"1").unwrap();
        write_atomic(&d.join("src/2"), b"2").unwrap();
        link_tree(&d.join("src"), &d.join("dst")).unwrap();
        copy_tree(&d.join("src"), &d.join("cp")).unwrap();
        for t in ["dst", "cp"] {
            assert_eq!(files_under(&d.join(t)).unwrap(), files_under(&d.join("src")).unwrap());
            assert_eq!(fs::read(d.join(t).join("x/1")).unwrap(), b"1");
        }
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_hard_linked_file_has_two_names() {
        let d = std::env::temp_dir().join(format!("fsx-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let (a, b) = (d.join("a"), d.join("b"));
        fs::write(&a, b"x").unwrap();
        assert_eq!(link_count(&a, &fs::symlink_metadata(&a).unwrap()), 1);
        fs::hard_link(&a, &b).unwrap();
        assert_eq!(link_count(&a, &fs::symlink_metadata(&a).unwrap()), 2);
        assert_eq!(
            link_count(&d.join("missing"), &fs::symlink_metadata(&b).unwrap()).max(1),
            link_count(&d.join("missing"), &fs::symlink_metadata(&b).unwrap())
        );
        fs::remove_dir_all(&d).unwrap();
    }
}

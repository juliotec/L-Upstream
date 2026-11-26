//! The object store: every compiled object by its key ([`crate::key`]), and by each base the headers that its compiles
//! read.
//!
//! ```text
//! <store>/objects/<k[..2]>/<key>.o       an object
//! <store>/manifests/<b[..2]>/<base>       what each compile of that base read: entries of
//!                                         "@ <key>" then "<sha1> <named header>" a line
//! ```
//!
//! One store may serve every work folder on the PC (`LONGSHOT_OBJ_STORE`), or be the work folder's own (`<work>/objects`).
//! The keys name no folder, so another work folder's object of the same key is the one this build would make.
//!
//! Safe against a stopped run and builds side by side: nothing is written in place. A file is written beside its place
//! under a name of this process's and renamed in, so a reader sees a whole file or none; two builds that make the same
//! object make the same bytes, and the second rename changes nothing. A work folder's objects are hard links to the
//! store's (a copy across drives), placed the same way, so the store's copy is never written through a work folder.
//! A manifest is read, added to and renamed in whole: two builds adding to one at once may lose one's entry, which costs
//! that object a compile later, never a wrong one.
//!
//! The store's clean-up is the builder's cache budget's (build_driver::storage): the objects not used for the longest
//! go first while the cache is over its size. A build marks each object it uses (its time, [`Store::used`]), so one in
//! use is never the oldest.

use build_core::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The store.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

/// The headers one compile of a base read, and the key they gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The object key.
    pub key: String,
    /// (named header, content hash), in the depfile's order.
    pub headers: Vec<(String, String)>,
}

/// How many entries a manifest keeps (the newest): a source built in a few configurations (headers edited back and
/// forth, two branches) finds each; more are rarely of use.
const KEEP_ENTRIES: usize = 8;

/// `bytes` written whole at `path` (beside it under a name of this call's own, then renamed in: the compile's threads
/// may write the same manifest at once).
fn write_whole(path: &Path, bytes: &[u8]) -> Result<()> {
    build_core::fsx::write_replace(path, bytes)
}

/// `from` placed at `to`: a hard link where it can be, else a copy, under a temporary name and renamed over whatever
/// `to` was (the old file's other links keep their bytes).
pub fn place(from: &Path, to: &Path) -> Result<()> {
    build_core::fsx::link_or_copy(from, to).map(|_| ())
}

/// The store's layout version: its folder under the store's root, so a later layout starts a store beside this one
/// and never misreads it.
pub const LAYOUT: &str = "v1";

impl Store {
    /// The store of this layout under `root` (`<root>/v1`), which outlives the builder: a work folder's or a per-user
    /// folder, never the builder's install.
    pub fn open(root: &Path) -> Store {
        Store::at(root.join(LAYOUT))
    }

    /// The layouts under `root` older than this builder's [`LAYOUT`] removed (`v0`, any `v<n>` below this one, and the
    /// unversioned layout's `objects/` and `manifests/` straight under `root`): their names. A newer builder's layout
    /// (a `v<n>` above this one), or anything else, is left.
    pub fn prune_layouts(root: &Path) -> Vec<String> {
        let ours: u32 = LAYOUT[1..].parse().unwrap_or(0);
        let mut gone = Vec::new();
        for e in fs::read_dir(root).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let older = match name.strip_prefix('v').and_then(|n| n.parse::<u32>().ok()) {
                Some(n) => n < ours,
                None => name == "objects" || name == "manifests",
            };
            if older && e.path().is_dir() && build_core::fsx::remove_tree(&e.path()).is_ok() {
                gone.push(name);
            }
        }
        gone
    }

    /// The store whose layout is at `root` itself (a prebuilt set, a test).
    pub fn at(root: impl Into<PathBuf>) -> Store {
        Store { root: root.into() }
    }

    /// Its folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the object of `key` is.
    pub fn object(&self, key: &str) -> PathBuf {
        self.root.join("objects").join(&key[..2.min(key.len())]).join(format!("{key}.o"))
    }

    fn manifest(&self, base: &str) -> PathBuf {
        self.root.join("manifests").join(&base[..2.min(base.len())]).join(base)
    }

    /// Whether it holds the object of `key`.
    pub fn has(&self, key: &str) -> bool {
        self.object(key).is_file()
    }

    /// The entries of a base's manifest, newest first (none when it has none, or it cannot be read).
    pub fn entries(&self, base: &str) -> Vec<Entry> {
        let Ok(text) = fs::read_to_string(self.manifest(base)) else { return Vec::new() };
        parse_manifest(&text)
    }

    /// Adds an entry to a base's manifest (or moves it to the front).
    pub fn add_entry(&self, base: &str, entry: &Entry) -> Result<()> {
        let mut all = self.entries(base);
        all.retain(|e| e != entry);
        all.insert(0, entry.clone());
        all.truncate(KEEP_ENTRIES);
        let mut text = String::new();
        for e in &all {
            text.push_str(&format!("@ {}\n", e.key));
            for (path, sha1) in &e.headers {
                text.push_str(&format!("{sha1} {path}\n"));
            }
            text.push_str(&format!("= {}\n", e.headers.len()));
        }
        write_whole(&self.manifest(base), text.as_bytes())
    }

    /// The object `file` (a compile's output) put in as `key`'s.
    pub fn put(&self, key: &str, file: &Path) -> Result<()> {
        let to = self.object(key);
        if to.is_file() {
            return Ok(()); // (another build made it: the same bytes)
        }
        place(file, &to)
    }

    /// The object of `key` placed at `to` (a work folder's object); false when the store does not have it.
    pub fn fetch(&self, key: &str, to: &Path) -> Result<bool> {
        let from = self.object(key);
        if !from.is_file() {
            return Ok(false);
        }
        place(&from, to)?;
        touch(&from);
        Ok(true)
    }

    /// Marks the object of `key` used now, so the clean-up keeps it (only when its mark is a day old: a build of a
    /// thousand objects opens none it marked yesterday).
    pub fn used(&self, key: &str) {
        let p = self.object(key);
        let old = fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(86_400));
        if old {
            touch(&p);
        }
    }

    /// Those objects out of the store (a link that failed with them): the next build compiles them again.
    pub fn forget(&self, keys: &[String]) {
        for key in keys {
            let _ = fs::remove_file(self.object(key));
        }
    }

    /// The manifests whose every object is gone, removed (after the cache's budget let objects go:
    /// build_driver::storage).
    pub fn drop_orphan_manifests(&self) {
        self.prune_manifests();
    }

    fn prune_manifests(&self) {
        let Ok(subs) = fs::read_dir(self.root.join("manifests")) else { return };
        for sub in subs.flatten() {
            let Ok(files) = fs::read_dir(sub.path()) else { continue };
            for f in files.flatten() {
                let Ok(text) = fs::read_to_string(f.path()) else { continue };
                if parse_manifest(&text).iter().all(|e| !self.has(&e.key)) {
                    let _ = fs::remove_file(f.path());
                }
            }
        }
    }
}

fn touch(path: &Path) {
    if let Ok(f) = fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
}

/// A manifest's entries: each ends with `= <its header count>`, so an entry cut short (a PC stopped as the file was
/// written) is no entry, never one with fewer headers that would match too much.
fn parse_manifest(text: &str) -> Vec<Entry> {
    let (mut out, mut cur): (Vec<Entry>, Option<Entry>) = (Vec::new(), None);
    for line in text.lines() {
        if let Some(key) = line.strip_prefix("@ ") {
            cur = Some(Entry { key: key.to_string(), headers: Vec::new() });
        } else if let Some(n) = line.strip_prefix("= ") {
            if let Some(e) = cur.take().filter(|e| n.parse::<usize>().is_ok_and(|n| n == e.headers.len())) {
                out.push(e);
            }
        } else if let (Some(e), Some((sha1, path))) = (cur.as_mut(), line.split_once(' ')) {
            e.headers.push((path.to_string(), sha1.to_string()));
        }
    }
    out
}

/// The object key a base has now: the first manifest entry whose headers all hash as they did (`hash` gives a named
/// header's hash now, `None` when it is gone).
pub fn lookup(entries: &[Entry], mut hash: impl FnMut(&str) -> Option<String>) -> Option<String> {
    entries
        .iter()
        .find(|e| e.headers.iter().all(|(path, sha1)| hash(path).as_deref() == Some(sha1.as_str())))
        .map(|e| e.key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_layouts_go_and_a_newer_one_stays() {
        let d = crate::testdir("store-layouts");
        for n in ["v0/objects", "v1/objects", "v2/objects", "objects/ab", "manifests/cd", "other"] {
            std::fs::create_dir_all(d.join(n)).unwrap();
        }
        let mut gone = Store::prune_layouts(&d);
        gone.sort();
        assert_eq!(gone, ["manifests", "objects", "v0"]);
        assert!(d.join("v1").is_dir() && d.join("v2").is_dir() && d.join("other").is_dir());
    }

    #[test]
    fn an_object_is_found_by_its_base_while_its_headers_are_as_they_were() {
        let d = crate::testdir("store");
        let s = Store::at(d.join("store"));
        let o = d.join("a.o");
        fs::write(&o, b"obj").unwrap();
        let e = Entry { key: "ab12".into(), headers: vec![("<root>/a.h".into(), "h1".into())] };
        s.put("ab12", &o).unwrap();
        s.add_entry("cd34", &e).unwrap();
        assert!(s.has("ab12"));
        let entries = s.entries("cd34");
        assert_eq!(lookup(&entries, |_| Some("h1".into())), Some("ab12".into()));
        assert_eq!(lookup(&entries, |_| Some("h2".into())), None, "a header changed: a miss");
        assert_eq!(lookup(&entries, |_| None), None, "a header gone: a miss");
        // (fetched into a work folder as a link of its own: the store's bytes stay)
        let w = d.join("work/a.o");
        assert!(s.fetch("ab12", &w).unwrap());
        assert_eq!(fs::read(&w).unwrap(), b"obj");
        assert!(!s.fetch("ffff", &w).unwrap());
        // (a second entry goes first; the same one again is not kept twice)
        let e2 = Entry { key: "ab13".into(), headers: vec![] };
        s.add_entry("cd34", &e2).unwrap();
        s.add_entry("cd34", &e).unwrap();
        assert_eq!(s.entries("cd34"), [e.clone(), e2]);
        // (an entry cut short is no entry)
        assert_eq!(
            parse_manifest("@ k1\nh1 <root>/a.h\n= 1\n@ k2\nh2 <root>/a.h\n"),
            [Entry { key: "k1".into(), headers: vec![("<root>/a.h".into(), "h1".into())] }]
        );
    }
}

//! Where a build reads and writes: the builder's tree, the work folder, the stores.
//!
//! ```text
//! <cache>/                         the per-user cache (LONGSHOT_CACHE; else %LOCALAPPDATA%\Longshot\Builder on
//!                                  Windows, $XDG_CACHE_HOME/longshot-builder or ~/.cache/longshot-builder elsewhere)
//!   work/<game folder's id>/       a game folder's work folder (--work names another)
//!     longshot-builder.lock          one build at a time (build_core::lock)
//!     exefs/                       the game's code modules as read (made again by every build)
//!     image/                       image.bin, image.json: the read stage's
//!     gen/                         the lift's (docs/dev/reference/builder.md, "The gen/ folder")
//!     build/                       the compile's: objects, the executable, the renderer, compile_state.json
//!     build_times.json             the builds' times
//!     build.log                    the last build's log
//!     game_folder.txt              the game folder it builds: a work folder whose game folder is gone is let go
//!   store/                         shared by every work folder (--store or LONGSHOT_STORE names another)
//!     compile/v1/objects, manifests  the object store (build_compile::store, its layout's version), by key
//!     lifts/<key>/                 whole lifts by key, hard-linked (the lift's own store)
//!   sources/<hash>/                a release's carried tree, unpacked (crate::sources), its tools linked in
//!   tools/<name>-<hash>/           each carried tool once, for every release that carries it
//!   runtimes/                      the upscalers' vendor files, by their SHA-256 (crate::runtimes)
//! ```
//!
//! The stores live outside the builder's install folder and every work folder, so a new builder release finds them
//! and reuses what its keys still match (each key is of exactly what made its object or unit, never of a path, the
//! builder's version or the build's options as a whole); and a game folder of its own per work folder, so two game
//! folders never wait on each other's lock or undo each other's lift.

use build_core::{Error, Result};
use std::path::{Path, PathBuf};

/// The per-user cache: `LONGSHOT_CACHE`, else the system's cache folder's `longshot-builder`. `get` gives each
/// variable as the system has it ([`crate::env_os`]: a home folder whose name is not Unicode is still one).
pub fn cache_root<S: Into<std::ffi::OsString>>(get: impl Fn(&str) -> Option<S>) -> PathBuf {
    let set = |k: &str| {
        let v: std::ffi::OsString = get(k)?.into();
        let v = match v.to_str() {
            Some(s) => s.trim().into(),
            None => v,
        };
        (!v.is_empty()).then(|| PathBuf::from(v))
    };
    if let Some(p) = set("LONGSHOT_CACHE") {
        return p;
    }
    if cfg!(windows) {
        let base = set("LOCALAPPDATA").or_else(|| set("USERPROFILE").map(|h| h.join("AppData").join("Local")));
        return base.unwrap_or_else(|| PathBuf::from(".")).join("Longshot").join("Builder");
    }
    set("XDG_CACHE_HOME")
        .or_else(|| set("HOME").map(|h| h.join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("longshot-builder")
}

/// A game folder's place as every command takes it: absolute, and on Linux with its links resolved (as far as it
/// exists), so a launcher's `--out` through a link and the program's own folder (`play`: its resolved path) are one
/// folder, with one work folder.
pub fn game_folder(out: &Path) -> PathBuf {
    let abs = std::path::absolute(out).unwrap_or_else(|_| out.to_path_buf());
    if cfg!(windows) {
        // (Windows: canonicalize gives a \\?\ path no other spelling compares to; its folders are not links)
        return abs;
    }
    let mut base = abs.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(base) {
            return tail.iter().rev().fold(real, |p: PathBuf, n| p.join(n));
        }
        match (base.parent(), base.file_name()) {
            (Some(up), Some(name)) => {
                tail.push(name.to_os_string());
                base = up;
            }
            _ => return abs,
        }
    }
}

/// The work folder of game folder `out` in the cache: named by a hash of its absolute path, so each game folder keeps
/// its own lift and objects, and finds them at its next build.
pub fn default_work(cache: &Path, out: &Path) -> PathBuf {
    work_of(cache, out, cfg!(windows))
}

/// [`default_work`], on Windows (`windows`) one name however the game folder's case is typed: its hash and its
/// readable part both in lower case. The name is in every link's command, so a readable part as typed
/// (`LONGSHOT-<hash>` for `c:\games\LONGSHOT`, `Longshot-<hash>` for `C:\Games\Longshot`: one folder there) changed
/// the link's key: both links ran again, the renderer came out with other bytes, and the shaders were prepared again.
fn work_of(cache: &Path, out: &Path, windows: bool) -> PathBuf {
    let abs = std::path::absolute(out).unwrap_or_else(|_| out.to_path_buf());
    let mut text = abs.to_string_lossy().replace('\\', "/");
    if windows {
        text = text.to_lowercase(); // (one folder, however its case is typed)
    }
    let id = build_core::key::fingerprint(text.trim_end_matches('/').as_bytes());
    let mut name: String = abs
        .file_name()
        .map(|n| n.to_string_lossy().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect())
        .unwrap_or_default();
    if windows {
        name.make_ascii_lowercase();
    }
    cache.join("work").join(if name.is_empty() { id } else { format!("{name}-{id}") })
}

/// The note in a work folder of the game folder it builds.
pub const GAME_NOTE: &str = "game_folder.txt";

/// Work folder `dir` of `works` (the cache's `work/`) removed, when no build holds it: moved aside while held, so a
/// build starting in it meanwhile is refused by the lock (try_lock: `work_folder_busy`, never a wait), or, once it is
/// moved, finds no folder and makes it again, never one being removed under it; then removed at leisure. Whether it
/// went. One a removal stopped part way left aside ([`GONE`]) is removed as it is. The cache's budget's
/// (crate::storage): a work folder holds the lift and the build of a game folder (gigabytes), made again by its next
/// build.
pub fn remove_work(works: &Path, dir: &Path) -> bool {
    let Some(name) = dir.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    if name.starts_with(GONE) {
        return build_core::fsx::remove_tree(dir).is_ok();
    }
    // (moved aside while it is held, so a build starting in it meanwhile is refused by the lock (try_lock:
    // `work_folder_busy`, never a wait), or, once it is moved, finds no folder and makes it again, never one being
    // removed under it; then removed at leisure. Windows refuses
    // to move a folder with the lock's file open in it: there it is emptied while held, and moved aside once let
    // go, which fails, so nothing is removed, if a build took it in the moment between)
    let Ok(lock) = build_core::lock::WorkLock::take(dir) else { return false };
    let aside = works.join(format!("{GONE}{name}-{}", std::process::id()));
    let moved = std::fs::rename(dir, &aside).is_ok();
    let emptied = moved
        || std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().filter(|e| e.file_name() != build_core::lock::FILE).all(|e| {
                let p = e.path();
                if p.is_dir() && !p.is_symlink() {
                    build_core::fsx::remove_tree(&p).is_ok()
                } else {
                    std::fs::remove_file(&p).is_ok()
                }
            })
        });
    drop(lock);
    let aside_now = moved || (emptied && std::fs::rename(dir, &aside).is_ok());
    aside_now && build_core::fsx::remove_tree(&aside).is_ok()
}

/// The start of a work folder's name while it is removed ([`remove_work`]).
pub const GONE: &str = ".gone-";

/// What the builders before this one left in the per-user cache folders, by name only (nothing else is looked at):
/// the Python builder's work beside the cache's own folders on Linux (`sa/`, `objcache/`, `exefs/`, `image/` in
/// `cache`), its Windows builder's caches (`<cache's parent>/longshot-builder-windows` on Linux; `%LOCALAPPDATA%\
/// LongshotBuilder` on Windows), removed: each one's place and size.
pub fn sweep_old_builders(cache: &Path, get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<(PathBuf, u64)> {
    let mut places: Vec<PathBuf> = ["sa", "objcache", "exefs", "image"].iter().map(|n| cache.join(n)).collect();
    if cfg!(windows) {
        if let Some(local) = get("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            places.push(PathBuf::from(local).join("LongshotBuilder"));
        }
    } else if let Some(parent) = cache.parent() {
        places.push(parent.join("longshot-builder-windows"));
    }
    let size = |p: &Path| {
        build_core::fsx::files_under(p)
            .map(|fs| fs.iter().filter_map(|f| std::fs::symlink_metadata(p.join(f)).ok()).map(|m| m.len()).sum())
            .unwrap_or(0)
    };
    places
        .into_iter()
        .filter(|p| p.is_dir() && !p.is_symlink())
        .filter_map(|p| {
            let n = size(&p);
            build_core::fsx::remove_tree(&p).ok().map(|()| (p, n))
        })
        .collect()
}

/// The stores' folder: `given`, else `LONGSHOT_STORE`, else the cache's `store/`.
pub fn store_root(given: Option<&Path>, cache: &Path, get: impl Fn(&str) -> Option<String>) -> PathBuf {
    given
        .map(Path::to_path_buf)
        .or_else(|| get("LONGSHOT_STORE").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| cache.join("store"))
}

/// A work folder's places.
#[derive(Clone, Debug)]
pub struct Work {
    /// The work folder.
    pub dir: PathBuf,
    /// The game's code modules as read.
    pub exefs: PathBuf,
    /// The image (the read stage's output, the lift's input).
    pub image: PathBuf,
    /// The lift's output.
    pub gen_dir: PathBuf,
    /// The compile's folder.
    pub build: PathBuf,
}

impl Work {
    /// The places of work folder `dir`.
    pub fn at(dir: impl Into<PathBuf>) -> Work {
        let dir = dir.into();
        Work {
            exefs: dir.join("exefs"),
            image: dir.join("image"),
            gen_dir: dir.join("gen"),
            build: dir.join("build"),
            dir,
        }
    }

    /// The builds' times.
    pub fn times(&self) -> PathBuf {
        self.dir.join("build_times.json")
    }

    /// The last build's log.
    pub fn log(&self) -> PathBuf {
        self.dir.join("build.log")
    }

    /// The finish's assets as made, laid out as the game folder (the finish puts them in).
    pub fn finish(&self) -> PathBuf {
        self.dir.join("finish")
    }
}

/// The stores' places.
#[derive(Clone, Debug)]
pub struct Stores {
    /// The stores' folder.
    pub root: PathBuf,
}

impl Stores {
    /// The object store's folder (build_compile's `Options::store`).
    pub fn compile(&self) -> PathBuf {
        self.root.join("compile")
    }

    /// The lifts' store (lift's `LiftOptions::store`).
    pub fn lifts(&self) -> PathBuf {
        self.root.join("lifts")
    }
}

/// Whether `dir` is the builder's tree: the game's sources the compile reads.
pub fn is_tree(dir: &Path) -> bool {
    ["host", "game", "gpu/vk", "third_party", "assets"].iter().all(|d| dir.join(d).is_dir())
}

/// The builder's tree: `given`, else `LONGSHOT_ROOT`, else the sources the program carries, unpacked into the cache
/// (a release's: crate::sources), else the first folder at or above the program's own that is one (a checkout's
/// program is under `target/<profile>/`).
pub fn tree_root(given: Option<&Path>, get: impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
    let named = given
        .map(Path::to_path_buf)
        .or_else(|| get("LONGSHOT_ROOT").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).map(PathBuf::from));
    if let Some(dir) = named {
        if is_tree(&dir) {
            return Ok(std::path::absolute(&dir).unwrap_or(dir));
        }
        return Err(Error::player(
            "tools_incomplete",
            format!(
                "{} is not Longshot's tools (no host/, game/, gpu/vk/, third_party/ and assets/ in it).",
                dir.display()
            ),
        ));
    }
    let carried = crate::sources::embedded();
    if !carried.is_empty() {
        return Ok(crate::sources::unpack(carried, &cache_root(crate::env_os))?.dir);
    }
    let exe = std::env::current_exe().ok().and_then(|e| std::fs::canonicalize(e).ok());
    if let Some(found) = exe.as_deref().and_then(|e| e.ancestors().skip(1).find(|d| is_tree(d))) {
        return Ok(found.to_path_buf());
    }
    Err(Error::player(
        "tools_incomplete",
        "This longshot-builder carries no sources (a developer's build): name a checkout of Longshot with --root or \
         LONGSHOT_ROOT, or get a release's longshot-builder.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_work_folder_goes_whole_unless_a_build_holds_it() {
        let d = crate::scratch("remove-work");
        let works = d.join("work");
        let w = works.join("game-1");
        std::fs::create_dir_all(w.join("gen")).unwrap();
        std::fs::write(w.join("gen/lift.key"), b"k").unwrap();
        let held = build_core::lock::WorkLock::take(&w).unwrap();
        assert!(!remove_work(&works, &w), "a build holds it");
        assert!(w.join("gen/lift.key").is_file());
        drop(held);
        // (flock belongs to the open file, and another test of this process may be forking a child right now: the
        // child holds the lock's file until its exec closes it, so the lock can outlive `held` for a moment)
        let t0 = std::time::Instant::now();
        while !remove_work(&works, &w) {
            assert!(t0.elapsed() < std::time::Duration::from_secs(5), "let go, it goes");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!w.exists());
        // (what a removal stopped part way left aside goes as it is)
        let aside = works.join(format!("{GONE}game-2-1"));
        std::fs::create_dir_all(&aside).unwrap();
        assert!(remove_work(&works, &aside) && !aside.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_old_builders_leftovers_go_by_name_only() {
        let d = crate::scratch("layout-old-builders");
        let cache = d.join("cache/longshot-builder");
        for f in ["sa/x.o", "objcache/a", "exefs/main", "image/image.bin", "work/w/build.log", "store/k"] {
            std::fs::create_dir_all(cache.join(f).parent().unwrap()).unwrap();
            std::fs::write(cache.join(f), b"12345").unwrap();
        }
        std::fs::create_dir_all(d.join("cache/longshot-builder-windows/obj")).unwrap();
        std::fs::write(d.join("cache/longshot-builder-windows/obj/a"), b"1").unwrap();
        let gone = sweep_old_builders(&cache, |_| None);
        let mut names: Vec<String> =
            gone.iter().map(|(p, _)| p.strip_prefix(&d).unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        if cfg!(windows) {
            assert_eq!(names.len(), 4);
        } else {
            assert_eq!(
                names,
                [
                    "cache/longshot-builder-windows",
                    "cache/longshot-builder/exefs",
                    "cache/longshot-builder/image",
                    "cache/longshot-builder/objcache",
                    "cache/longshot-builder/sa"
                ]
            );
        }
        assert_eq!(gone.iter().map(|g| g.1).sum::<u64>(), if cfg!(windows) { 20 } else { 21 });
        assert!(cache.join("work/w/build.log").is_file() && cache.join("store/k").is_file(), "the builder's own stay");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_work_folder_per_game_folder_and_the_stores_beside_them() {
        let env = |k: &str| (k == "HOME").then(|| "/home/p".to_string());
        let cache = cache_root(env);
        if !cfg!(windows) {
            assert_eq!(cache, Path::new("/home/p/.cache/longshot-builder"));
        }
        let a = default_work(&cache, Path::new("/games/Longshot"));
        let b = default_work(&cache, Path::new("/games/Longshot2"));
        assert_ne!(a, b);
        assert_eq!(a, default_work(&cache, Path::new("/games/Longshot/")));
        let name = a.file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(a.starts_with(cache.join("work")) && name.starts_with("longshot-"));
        // (Windows: one folder however its case is typed, so the links' commands, which name it, are the same)
        let typed = |p: &str| work_of(&cache, Path::new(p), true);
        assert_eq!(typed(r"C:\Games\Jösé\Longshot"), typed(r"c:\GAMES\Jösé\LONGSHOT"));
        let unix = |p: &str| work_of(&cache, Path::new(p), false);
        assert_ne!(unix("/games/Longshot"), unix("/games/LONGSHOT"), "two folders elsewhere");
        assert_eq!(store_root(None, &cache, |_| None), cache.join("store"));
        assert_eq!(store_root(None, &cache, |_| Some("/s".into())), Path::new("/s"));
        assert_eq!(cache_root(|k| (k == "LONGSHOT_CACHE").then(|| "/c".to_string())), Path::new("/c"));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            // (a home folder whose name is not UTF-8: still the cache's place)
            let home = std::ffi::OsString::from_vec(b"/home/\xffp".to_vec());
            let cache = cache_root(|k| (k == "HOME").then(|| home.clone()));
            assert_eq!(cache, Path::new(&home).join(".cache/longshot-builder"));
        }
    }
}

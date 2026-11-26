//! Lifts kept for other work folders: `<store>/<key>/` holds a gen/ folder's files, hard-linked (or copied where links
//! cannot be made), and `complete` last, so a lift that stopped half way is never taken. The lift writes every file
//! by a rename ([`build_core::fsx::write_atomic`]), so a link shared with the store is never changed in place.
//!
//! The store marks each lift's last use (`complete`'s time, made new when a lift is taken); the builder's cache budget
//! (build_driver::storage) lets the least recently used go ([`listed`], [`let_go`]) with the rest of the cache, never
//! one a build is using now ([`in_use`]). Profiles whose code mods differ and a version switched back to keep their
//! lifts, where a fixed count made each switch a lift again (5 s, and the units that changed compiled again).
//!
//! The store is shared by every work folder, so two builds of two game folders can save, prune and restore at once:
//! each does it holding the store's lock (`<store>/.lock`, waited for), and writes a lift being saved under a name of
//! its own (`<key>.<pid>.part`). What a save stopped part way left (another `.part`: under the lock, no one else is
//! writing one) goes at the next save. A build holds `<store>/<key>.use` (shared) for the lift it takes or makes, from
//! its key on, so another build's prune between its look and its take leaves it.

use build_core::{IoContext, Result, fsx};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const COMPLETE: &str = "complete";
const LOCK: &str = ".lock";
const USE: &str = ".use";

/// The store held until the file is dropped, waiting for another build that holds it.
fn held(store: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(store).at(store)?;
    let path = store.join(LOCK);
    let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).at(&path)?;
    file.lock().at(&path)?;
    Ok(file)
}

fn use_file(store: &Path, key: &str) -> PathBuf {
    store.join(format!("{key}{USE}"))
}

/// The lift of `key` in use by this build until this is dropped: no prune lets it go meanwhile.
#[derive(Debug)]
pub struct InUse {
    _file: std::fs::File,
}

/// The lift of `key` marked in use (`<store>/<key>.use`, held shared: every build using it holds it).
pub fn in_use(store: &Path, key: &str) -> Result<InUse> {
    std::fs::create_dir_all(store).at(store)?;
    // (under the store's lock, as let_go removes a mark under it: never a mark taken between its check and its removal,
    // which on Windows leaves the file delete-pending and the next build's open of it refused)
    let _held = held(store)?;
    let path = use_file(store, key);
    let file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).at(&path)?;
    file.lock_shared().at(&path)?;
    Ok(InUse { _file: file })
}

/// Whether a build holds the lift of `key` ([`in_use`]); a mark no one holds is taken out with its lift.
pub fn used_now(store: &Path, key: &str) -> bool {
    let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).open(use_file(store, key)) else { return false };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// The lift of `key` into `gen` from the store, if it has it whole: whether it did.
pub fn restore(store: &Path, key: &str, gen_dir: &Path) -> Result<bool> {
    let dir = store.join(key);
    if !dir.join(COMPLETE).is_file() {
        return Ok(false);
    }
    let _held = held(store)?;
    // (again, held: a prune may have taken it meanwhile)
    if !dir.join(COMPLETE).is_file() {
        return Ok(false);
    }
    for entry in std::fs::read_dir(gen_dir).at(gen_dir)? {
        let name = entry.at(gen_dir)?.file_name().to_string_lossy().into_owned();
        if name.starts_with("lifted_") && name.ends_with(".ll") {
            fsx::remove_file(&gen_dir.join(&name))?;
        }
    }
    for rel in fsx::files_under(&dir)? {
        if rel.as_os_str() != COMPLETE {
            fsx::link_or_copy(&dir.join(&rel), &gen_dir.join(&rel))?;
        }
    }
    // (touched: kept by last use)
    fsx::write_atomic(&dir.join(COMPLETE), key.as_bytes())?;
    Ok(true)
}

/// The lift in `gen` (of `key`) kept (what lets it go: the builder's cache budget, [`let_go`]).
pub fn save(store: &Path, key: &str, gen_dir: &Path) -> Result<()> {
    let dir = store.join(key);
    let _held = held(store)?;
    if dir.join(COMPLETE).is_file() {
        // (lifted again, forced: its use is now)
        fsx::write_atomic(&dir.join(COMPLETE), key.as_bytes())?;
        return Ok(());
    }
    // (every .part is a save that stopped: no one else writes one while the lock is held)
    for e in std::fs::read_dir(store).at(store)?.flatten() {
        if e.file_name().to_string_lossy().ends_with(".part") {
            fsx::remove_tree(&e.path())?;
        }
    }
    let part = store.join(format!("{key}.{}.part", std::process::id()));
    fsx::link_tree(gen_dir, &part)?;
    fsx::write_atomic(&part.join(COMPLETE), key.as_bytes())?;
    fsx::remove_tree(&dir)?;
    std::fs::rename(&part, &dir).at(&dir)?;
    Ok(())
}

/// A lift in the store: its key, its folder, its last use, and the bytes it alone holds.
#[derive(Clone, Debug)]
pub struct Kept {
    /// Its key.
    pub key: String,
    /// Its folder.
    pub dir: PathBuf,
    /// Its last use (`complete`'s time).
    pub used: SystemTime,
    /// The bytes no lift used after it shares.
    pub bytes: u64,
}

/// The store's lifts, the most recently used first, each with the bytes no lift used after it shares (the files a
/// lift kept unchanged are links of one file: letting the older go frees nothing of them). None when there is no store.
pub fn listed(store: &Path) -> Result<Vec<Kept>> {
    let mut kept: Vec<Kept> = std::fs::read_dir(store)
        .at(store)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let used = std::fs::metadata(e.path().join(COMPLETE)).and_then(|m| m.modified()).ok()?;
            let key = e.file_name().to_string_lossy().into_owned();
            Some(Kept { key, dir: e.path(), used, bytes: 0 })
        })
        .collect();
    kept.sort_by(|a, b| b.used.cmp(&a.used).then_with(|| a.key.cmp(&b.key)));
    let mut seen = std::collections::HashSet::new();
    for k in &mut kept {
        for rel in fsx::files_under(&k.dir)? {
            let Ok(m) = std::fs::symlink_metadata(k.dir.join(&rel)) else { continue };
            #[cfg(unix)]
            let first = seen.insert((std::os::unix::fs::MetadataExt::dev(&m), std::os::unix::fs::MetadataExt::ino(&m)));
            #[cfg(not(unix))]
            let first = seen.insert((k.key.clone(), rel));
            if first {
                k.bytes += m.len();
            }
        }
    }
    Ok(kept)
}

/// The lift of `key` let go, under the store's lock: `false` (nothing removed) when a build holds it ([`in_use`]). A
/// save that stopped part way (`<key>.<pid>.part`, `key` its whole name) goes the same way. The marks of lifts gone
/// that no build holds go with it.
pub fn let_go(store: &Path, key: &str) -> Result<bool> {
    let _held = held(store)?;
    let part = key.ends_with(".part");
    if key.is_empty() || key.contains(['/', '\\']) || (!part && used_now(store, key)) {
        return Ok(false);
    }
    fsx::remove_tree(&store.join(key))?;
    let _ = std::fs::remove_file(use_file(store, key));
    // (the marks of lifts gone that no build holds)
    for e in std::fs::read_dir(store).at(store)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(key) = name.strip_suffix(USE)
            && !key.is_empty()
            && !store.join(key).is_dir()
            && !used_now(store, key)
        {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), |t| root.join(t));
        let d = target.join("test-scratch").join(format!("lift-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A store of lifts (key, a gen/ folder of 997 bytes, its last use that many days ago), all saved before any
    /// is aged.
    fn store_of(d: &Path, lifts: &[(&str, u64)]) -> PathBuf {
        let store = d.join("store");
        for (key, _) in lifts {
            let gen_dir = d.join(format!("gen-{key}"));
            std::fs::create_dir_all(&gen_dir).unwrap();
            std::fs::write(gen_dir.join("lifted_0.ll"), vec![b'x'; 997]).unwrap();
            save(&store, key, &gen_dir).unwrap();
        }
        for (key, days) in lifts {
            aged(&store, key, *days);
        }
        store
    }

    fn aged(store: &Path, key: &str, days_ago: u64) {
        let t = SystemTime::now() - std::time::Duration::from_secs(days_ago * 86_400);
        std::fs::File::options().append(true).open(store.join(key).join(COMPLETE)).unwrap().set_modified(t).unwrap();
    }

    fn lifts(store: &Path) -> Vec<String> {
        let mut left: Vec<String> = std::fs::read_dir(store)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| store.join(n).is_dir())
            .collect();
        left.sort();
        left
    }

    #[test]
    fn a_save_sweeps_what_a_stopped_save_left_and_a_restore_takes_it_whole() {
        let d = scratch("save");
        let (store, gen_dir) = (d.join("store"), d.join("gen"));
        std::fs::create_dir_all(&gen_dir).unwrap();
        std::fs::write(gen_dir.join("lifted_0.ll"), b"a").unwrap();
        // (a save another process stopped part way)
        std::fs::create_dir_all(store.join("k0.99999.part")).unwrap();
        save(&store, "k1", &gen_dir).unwrap();
        assert_eq!(lifts(&store), ["k1"]);
        std::fs::remove_file(gen_dir.join("lifted_0.ll")).unwrap();
        assert!(restore(&store, "k1", &gen_dir).unwrap());
        assert_eq!(std::fs::read(gen_dir.join("lifted_0.ll")).unwrap(), b"a");
        assert!(!restore(&store, "k9", &gen_dir).unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn they_are_listed_by_last_use_and_one_in_use_is_not_let_go() {
        let d = scratch("let-go");
        let store = store_of(&d, &[("old", 30), ("held", 20), ("new", 1)]);
        let gen_dir = d.join("gen-restore");
        std::fs::create_dir_all(&gen_dir).unwrap();
        let keys = |store: &Path| listed(store).unwrap().into_iter().map(|k| k.key).collect::<Vec<_>>();
        assert_eq!(keys(&store), ["new", "held", "old"]);
        // (taken again: its use is now)
        assert!(restore(&store, "old", &gen_dir).unwrap());
        assert_eq!(keys(&store), ["old", "new", "held"]);
        let held = in_use(&store, "held").unwrap();
        assert!(!let_go(&store, "held").unwrap(), "a build holds it");
        assert!(let_go(&store, "new").unwrap());
        assert_eq!(lifts(&store), ["held", "old"]);
        drop(held);
        // (a lock is an open file description: a test running beside this one that forks (the compiler tests) gives a
        // child a copy of it for the moments before its exec, so the lock lets go a few microseconds after the drop,
        // not at it: tried again until then)
        let mut gone = false;
        for _ in 0..200 {
            gone = let_go(&store, "held").unwrap();
            if gone {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(gone);
        assert!(!store.join("held.use").exists(), "its mark goes with it");
        assert!(!let_go(&store, "../x").unwrap(), "a name outside the store: nothing");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_two_lifts_share_counts_once() {
        let d = scratch("shared");
        let store = d.join("store");
        let gen_dir = d.join("gen");
        std::fs::create_dir_all(&gen_dir).unwrap();
        std::fs::write(gen_dir.join("big.c"), vec![b'x'; 5000]).unwrap();
        std::fs::write(gen_dir.join("lifted_0.ll"), b"1").unwrap();
        save(&store, "one", &gen_dir).unwrap();
        aged(&store, "one", 1);
        // (the next lift writes only what changed: big.c stays one file, linked into both)
        fsx::write_atomic(&gen_dir.join("lifted_0.ll"), b"2").unwrap();
        save(&store, "two", &gen_dir).unwrap();
        let kept = listed(&store).unwrap();
        let bytes: Vec<(&str, u64)> = kept.iter().map(|k| (k.key.as_str(), k.bytes)).collect();
        if cfg!(unix) {
            assert_eq!(bytes, [("two", 5000 + 1 + 3), ("one", 1 + 3)]);
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

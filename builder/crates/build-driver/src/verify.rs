//! `longshot-builder verify`: a game folder checked against what the builder put there, and with `--repair` the
//! damaged files put back.
//!
//! The installed note (the record's `files`, `longshot_build.json`: [`folder::FILES_KEY`]; an earlier builder's own
//! `.longshot_installed.json`, [`folder::INSTALLED_NOTE`], until the next write merges it) lists every file the finish
//! put in the folder with its size and SHA-256, written at the swap; [`verify`] checks each against it: missing, changed (another
//! size, else another SHA-256), or as put there. Every other file is the game's or the player's ([`USER_DATA`]: the
//! saves, the settings, the mods, the logs, the caches the game writes), never read nor touched, or one of neither
//! (an `extra`, said and left as it is). [`repair`] puts each damaged file back from the work folder when a copy
//! there still has the noted bytes; what it can't is left to an update (longshot-builder's `verify --repair`), which
//! copies in every file of the note not as it was put there.

use crate::folder::{self, Noted, Record};
use build_core::report::{Event, Reporter};
use build_core::{Result, fsx};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What the game and the player write in a game folder, by its name there (a file, or a folder and all in it):
/// `verify` never checks, repairs nor counts it as extra. The saves (`user/`), the mods (`mods/`, `mods.json`, the
/// mods' own cache), the settings (`longshot_options.ini`, `language.txt`, `longshot_env.txt`, the mouse's), the
/// caches and logs the game makes (`cache/`, `logs/`, its bug reports), the performance statistics.
pub const USER_DATA: [&str; 15] = [
    "user",
    "mods",
    "mods.json",
    "modcache",
    "cache",
    "logs",
    "bug-reports",
    "perf_pending",
    "perf_sent",
    "longshot_perf.ini",
    "longshot_options.ini",
    "longshot_env.txt",
    "language.txt",
    "mouse_sens.txt",
    "saves-moved.txt",
];

/// The builder's own files that no hash covers: the note itself, the folder's lock, the record `update` reads (by any of
/// its names, an earlier builder's among them).
const OWN: [&str; 5] = [
    folder::INSTALLED_NOTE,
    build_core::lock::GAME_FILE,
    folder::RECORD_FILES[0],
    folder::RECORD_FILES[1],
    folder::RECORD_FILES[2],
];

/// Whether `rel` (a name in the game folder, `/` between folders) is the game's or the player's ([`USER_DATA`]), or
/// a file the game is writing whole (`<name>.hosnew`, beside a setting).
pub fn is_user_data(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or(rel);
    USER_DATA.contains(&first) || rel.ends_with(".hosnew")
}

/// What [`verify`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verified {
    /// The builder's files checked.
    pub checked: usize,
    /// The builder's files that are not there.
    pub missing: Vec<String>,
    /// The builder's files of another size or SHA-256 than it put there.
    pub changed: Vec<String>,
    /// The builder's files noted without a SHA-256 (a builder before): an update notes them.
    pub unchecked: Vec<String>,
    /// Files of neither the builder nor the game or the player's data.
    pub extra: Vec<String>,
    /// The game's and the player's data there, by name (a folder's ends in `/`).
    pub excluded: Vec<String>,
}

impl Verified {
    /// The files to put back: missing or changed.
    pub fn damaged(&self) -> Vec<String> {
        self.missing.iter().chain(&self.changed).cloned().collect()
    }

    /// Whether every file the builder put there is checked and as it put it.
    pub fn intact(&self) -> bool {
        self.missing.is_empty() && self.changed.is_empty() && self.unchecked.is_empty()
    }
}

/// The SHA-256 of a file, or `None` when it can't be read (a damaged one, said as changed).
fn sha256(path: &Path) -> Option<String> {
    crate::sha256_file(path).ok().map(|(sha, _)| sha)
}

/// Whether the file at `path` has the noted size and SHA-256.
fn as_noted(path: &Path, n: &Noted) -> bool {
    let size = std::fs::symlink_metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len());
    size == Some(n.size) && n.sha256.is_some() && sha256(path) == n.sha256
}

/// Game folder `out` checked against its installed note, on `jobs` threads (the files' hashes side by side). Reads
/// only: nothing in the folder is written, not even its lock. A folder this builder did not build: `not_built_here`.
pub fn verify(out: &Path, jobs: usize) -> Result<Verified> {
    Record::read(out)?;
    let (files, trees) = folder::noted(out);
    let mut v = Verified::default();
    let mut hash: Vec<&Noted> = Vec::new();
    for n in &files {
        if is_user_data(&n.rel) {
            continue; // (never the builder's to check, whatever a note says)
        }
        v.checked += 1;
        match std::fs::symlink_metadata(out.join(&n.rel)) {
            Ok(m) if m.is_file() => {
                if m.len() != n.size {
                    v.changed.push(n.rel.clone());
                } else if n.sha256.is_none() {
                    v.unchecked.push(n.rel.clone());
                } else {
                    hash.push(n);
                }
            }
            _ => v.missing.push(n.rel.clone()),
        }
    }
    // (the largest first, each thread taking the next: the game's executable is most of the bytes)
    hash.sort_by_key(|n| std::cmp::Reverse(n.size));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let bad = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, hash.len().max(1)) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(n) = hash.get(i) else { break };
                    if sha256(&out.join(&n.rel)) != n.sha256 {
                        bad.lock().unwrap_or_else(|p| p.into_inner()).push(n.rel.clone());
                    }
                }
            });
        }
    });
    v.changed.extend(bad.into_inner().unwrap_or_else(|p| p.into_inner()));
    v.changed.sort();
    // (the rest of the folder: the player's data by name, the builder's own, and what is neither)
    let noted: BTreeSet<&str> = files.iter().map(|n| n.rel.as_str()).collect();
    let trees: BTreeSet<&str> = trees.iter().map(|t| t.trim_end_matches('/')).collect();
    let mut top: Vec<_> = std::fs::read_dir(out).map_err(|e| build_core::Error::io(out, e))?.flatten().collect();
    top.sort_by_key(|e| e.file_name());
    for e in top {
        let name = e.file_name().to_string_lossy().into_owned();
        let dir = e.file_type().is_ok_and(|t| t.is_dir());
        if is_user_data(&name) {
            v.excluded.push(if dir { format!("{name}/") } else { name });
            continue;
        }
        // (the builder's own, which no hash covers; what the finish writes after the swap, noted from this builder on;
        // a dump's romfs/, linked in whole)
        if OWN.contains(&name.as_str()) || folder::WRITTEN.contains(&name.as_str()) || trees.contains(name.as_str()) {
            if !dir && folder::WRITTEN.contains(&name.as_str()) && !noted.contains(name.as_str()) {
                v.unchecked.push(name);
            }
            continue;
        }
        if dir {
            for rel in fsx::files_under(&e.path())? {
                let rel = format!("{name}/{}", rel.to_string_lossy().replace('\\', "/"));
                if !noted.contains(rel.as_str()) && !is_user_data(&rel) {
                    v.extra.push(rel);
                }
            }
        } else if !noted.contains(name.as_str()) {
            v.extra.push(name);
        }
    }
    Ok(v)
}

/// Where a copy of the game folder's `rel` may still be in work folder `work`: the finish's assets as made
/// (`finish/`), the compile's outputs (`build/`: the game, the renderer, their libraries), the image (`image/`).
fn copies(work: &Path, rel: &str) -> Vec<PathBuf> {
    let mut at = vec![work.join("finish").join(rel)];
    if !rel.contains('/') {
        at.push(work.join("build").join(rel));
        at.push(work.join("image").join(rel));
    }
    at
}

/// The damaged files of `v` put back from work folder `work`, each from a copy there that still has the bytes the
/// note lists, holding the game folder (`game_folder_busy` while a build writes it): the names put back. Nothing else
/// in the folder is written; what has no such copy is left for an update.
pub fn repair(out: &Path, work: &Path, v: &Verified, r: &Reporter) -> Result<Vec<String>> {
    let _held = build_core::lock::GameLock::take(out)?;
    let (files, _) = folder::noted(out);
    let mut repaired = Vec::new();
    for rel in v.damaged() {
        let Some(n) = files.iter().find(|n| n.rel == rel) else { continue };
        if is_user_data(&rel) {
            continue;
        }
        if let Some(src) = copies(work, &rel).into_iter().find(|c| as_noted(c, n)) {
            folder::put_back(out, &rel, &src)?;
            r.log(format!("{rel}: put back from {}", src.display()));
            repaired.push(rel);
        }
    }
    Ok(repaired)
}

/// What `v` found, said: a warning for each damaged file, then the `verify` event.
pub fn say(v: &Verified, repaired: &[String], r: &Reporter) {
    for rel in &v.missing {
        r.warn("game_file_missing", format!("{rel} is missing from the game folder. Repair it, or update the game."));
    }
    for rel in &v.changed {
        r.warn(
            "game_file_changed",
            format!("{rel} is not the file the builder put in the game folder. Repair it, or update the game."),
        );
    }
    if !v.unchecked.is_empty() {
        r.warn(
            "game_files_unchecked",
            format!(
                "{} files were put in the game folder by an older builder, which noted no SHA-256 for them: an update \
                 notes them, and then they can be checked.",
                v.unchecked.len()
            ),
        );
    }
    r.send(Event::Verify {
        ok: v.intact(),
        checked: v.checked,
        missing: v.missing.clone(),
        changed: v.changed.clone(),
        unchecked: v.unchecked.clone(),
        extra: v.extra.clone(),
        excluded: v.excluded.clone(),
        repaired: repaired.to_vec(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A built game folder: its record, the files a finish put there (the game, an asset in a folder, a library),
    /// the player's data beside them, and the work folder's copies of the assets and the game.
    fn built(name: &str) -> (PathBuf, PathBuf) {
        let d = crate::scratch(name);
        let (src, out, work) = (d.join("src"), d.join("game"), d.join("work"));
        for (rel, bytes) in [("longshot", &b"the game"[..]), ("texts/en.txt", b"words"), ("libSDL2-2.0.so.0", b"sdl")] {
            std::fs::create_dir_all(src.join(rel).parent().unwrap()).unwrap();
            std::fs::write(src.join(rel), bytes).unwrap();
        }
        let files: Vec<(PathBuf, PathBuf)> =
            ["longshot", "texts/en.txt", "libSDL2-2.0.so.0"].iter().map(|r| (src.join(r), out.join(r))).collect();
        folder::install(&out, &files, &[], &[]).unwrap();
        Record::new("v1", &crate::GameSource::Nup(d.join("g.nup")), &[]).write(&out).unwrap();
        // (the player's: a save, the settings, the game's caches and logs)
        for (rel, bytes) in [
            ("user/save/slot0.bin", &b"save"[..]),
            ("longshot_options.ini", b"[video]"),
            ("cache/longshot_programs.bin", b"programs"),
            ("logs/longshot_log.txt", b"log"),
            ("language.txt", b"fr\n"),
            ("mods/a/mod.json", b"{}"),
        ] {
            std::fs::create_dir_all(out.join(rel).parent().unwrap()).unwrap();
            std::fs::write(out.join(rel), bytes).unwrap();
        }
        std::fs::create_dir_all(work.join("finish/texts")).unwrap();
        std::fs::write(work.join("finish/texts/en.txt"), b"words").unwrap();
        std::fs::create_dir_all(work.join("build")).unwrap();
        std::fs::write(work.join("build/longshot"), b"the game").unwrap();
        (out, work)
    }

    fn user_files(out: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
        ["user/save/slot0.bin", "longshot_options.ini", "cache/longshot_programs.bin", "logs/longshot_log.txt"]
            .iter()
            .map(|r| {
                let p = out.join(r);
                let t = std::fs::metadata(&p).unwrap().modified().unwrap();
                (p.clone(), std::fs::read(&p).unwrap(), t)
            })
            .collect()
    }

    #[test]
    fn an_intact_folder_is_intact_and_the_player_s_data_is_excluded() {
        let (out, _) = built("verify-intact");
        let v = verify(&out, 4).unwrap();
        assert!(v.intact(), "{v:?}");
        assert_eq!(v.checked, 3);
        assert!(v.extra.is_empty(), "{v:?}");
        assert_eq!(v.excluded, ["cache/", "language.txt", "logs/", "longshot_options.ini", "mods/", "user/"]);
    }

    #[test]
    fn a_changed_file_a_missing_one_and_an_extra_one_are_said() {
        let (out, _) = built("verify-damaged");
        std::fs::write(out.join("texts/en.txt"), b"wordz").unwrap(); // (the same size: the hash says it)
        std::fs::write(out.join("libSDL2-2.0.so.0"), b"a longer library").unwrap();
        std::fs::remove_file(out.join("longshot")).unwrap();
        std::fs::write(out.join("reshade.ini"), b"x").unwrap();
        // (the player's data changed: none of the builder's business)
        std::fs::write(out.join("user/save/slot0.bin"), b"another save").unwrap();
        let v = verify(&out, 2).unwrap();
        assert!(!v.intact());
        assert_eq!(v.missing, ["longshot"]);
        assert_eq!(v.changed, ["libSDL2-2.0.so.0", "texts/en.txt"]);
        assert_eq!(v.extra, ["reshade.ini"]);
        let lines = say_lines(&v);
        assert!(lines.iter().any(|l| l.contains("\"code\":\"game_file_missing\"")), "{lines:?}");
        assert!(lines.last().unwrap().contains("\"type\":\"verify\""), "{lines:?}");
    }

    fn say_lines(v: &Verified) -> Vec<String> {
        #[derive(Clone, Default)]
        struct Buf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        say(v, &[], &Reporter::to(build_core::report::Format::Json, Box::new(buf.clone())));
        String::from_utf8(buf.0.lock().unwrap().clone()).unwrap().lines().map(String::from).collect()
    }

    #[test]
    fn a_repair_puts_back_what_the_work_folder_still_has_and_touches_nothing_of_the_player_s() {
        let (out, work) = built("verify-repair");
        let before = user_files(&out);
        std::fs::write(out.join("texts/en.txt"), b"wordz").unwrap();
        std::fs::remove_file(out.join("longshot")).unwrap();
        std::fs::write(out.join("libSDL2-2.0.so.0"), b"x").unwrap(); // (no copy of it in the work folder)
        std::fs::write(out.join("reshade.ini"), b"x").unwrap();
        let v = verify(&out, 4).unwrap();
        let r = Reporter::to(build_core::report::Format::Json, Box::new(std::io::sink()));
        let mut repaired = repair(&out, &work, &v, &r).unwrap();
        repaired.sort();
        assert_eq!(repaired, ["longshot", "texts/en.txt"]);
        assert_eq!(std::fs::read(out.join("longshot")).unwrap(), b"the game");
        let after = verify(&out, 4).unwrap();
        assert_eq!((after.missing.len(), after.changed.clone()), (0, vec!["libSDL2-2.0.so.0".to_string()]));
        // (an update after keeps what was put back: the note has its stat)
        assert!(folder::ours(&out, &out.join("longshot")));
        assert_eq!(user_files(&out), before, "the player's files as they were");
        assert_eq!(std::fs::read(out.join("reshade.ini")).unwrap(), b"x", "an extra file left as it is");
        assert!(!out.join("longshot.new").exists());
    }

    #[test]
    fn a_copy_in_the_work_folder_that_is_not_the_noted_one_is_not_put_back() {
        let (out, work) = built("verify-stale");
        std::fs::remove_file(out.join("longshot")).unwrap();
        std::fs::write(work.join("build/longshot"), b"a newer game").unwrap();
        let v = verify(&out, 1).unwrap();
        let r = Reporter::to(build_core::report::Format::Json, Box::new(std::io::sink()));
        assert!(repair(&out, &work, &v, &r).unwrap().is_empty());
        assert!(!out.join("longshot").exists(), "left for an update");
    }

    #[test]
    fn an_older_builder_s_note_is_unchecked_not_damaged() {
        let (out, _) = built("verify-old-note");
        // (an older builder's: its own file beside a record with no files, SHA-1s in it)
        let record = std::fs::read_to_string(out.join(folder::RECORD_FILE)).unwrap();
        let mut rec: serde_json::Value = serde_json::from_str(&record).unwrap();
        let files = rec.as_object_mut().unwrap().remove(folder::FILES_KEY).unwrap();
        std::fs::write(out.join(folder::RECORD_FILE), serde_json::to_vec(&rec).unwrap()).unwrap();
        let mut old = serde_json::Map::new();
        for (k, e) in files.as_object().unwrap() {
            old.insert(k.clone(), serde_json::json!({"sha1": "0".repeat(40), "stat": e["stat"]}));
        }
        std::fs::write(out.join(folder::INSTALLED_NOTE), serde_json::to_vec(&old).unwrap()).unwrap();
        let v = verify(&out, 2).unwrap();
        assert_eq!((v.unchecked.len(), v.missing.len(), v.changed.len()), (3, 0, 0));
        assert!(!v.intact());
    }

    #[test]
    fn a_folder_the_builder_did_not_build_is_not_built_here() {
        let d = crate::scratch("verify-not-built");
        assert_eq!(verify(&d, 1).unwrap_err().code(), "not_built_here");
    }
}

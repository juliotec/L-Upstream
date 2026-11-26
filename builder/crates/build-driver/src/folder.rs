//! The game folder: its checks before a build, the files the finish puts in it, and what it keeps for the next build.
//!
//! The new files are copied in beside the old ones as `<name>.new` and swapped in only once all are complete: a build
//! stopped while copying, or a full disk, once left a half-written 460 MB exe and no game to play; the old game stays
//! playable until the swap. A file the last finish put there, still as it was, with the same bytes as the new one, is
//! not copied again (the installed note): an update with nothing changed would copy the same exe again, 4-8 s and
//! more where an antivirus reads each new exe.

use crate::options::{CompileShaders, GameSource, LANGUAGES, Upscaler};
use build_core::{Error, Result, fsx};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A path as compared: its longest part that exists at its real place (links and `..` resolved), the rest (folders a
/// build would make) after it as given; on Windows without regard to case, as its file systems take it. (A game
/// folder not made yet, under a link to the dump, is inside the dump.)
fn compared(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut tail = Vec::new();
    let real = loop {
        if let Ok(r) = std::fs::canonicalize(base) {
            break r;
        }
        let last = base.components().next_back().map(|c| c.as_os_str().to_os_string());
        match (base.parent(), last) {
            (Some(up), Some(name)) => {
                tail.push(name);
                base = up;
            }
            _ => break abs.clone(),
        }
    };
    let mut p = real;
    for name in tail.into_iter().rev() {
        if name == ".." {
            p.pop();
        } else if name != "." {
            p.push(name);
        }
    }
    if cfg!(windows) { PathBuf::from(p.to_string_lossy().to_lowercase()) } else { p }
}

fn overlap(a: &Path, b: &Path) -> bool {
    let (a, b) = (compared(a), compared(b));
    a.starts_with(&b) || b.starts_with(&a)
}

/// Whether `inner` is `outer` or inside it.
fn within(inner: &Path, outer: &Path) -> bool {
    compared(inner).starts_with(compared(outer))
}

/// The builder's own folder in [`check_places`]: the only one a game folder may be inside.
pub const TOOLS: &str = "tools folder";

/// Where a build writes is apart from what it must not touch: the game folder `out` from the builder's own folders
/// (`builder`: the work folder, the store, the tools: a clean-up of one would take the other), and both `out` and
/// the work folder from the player's game files (a dump's folders, either way round: linking its romfs into a game
/// folder inside it, or making the work folder's exefs again in it, would delete the dump's own files; the .nup
/// inside either).
pub fn check_places(out: &Path, work: &Path, builder: &[(&str, &Path)], game: &GameSource) -> Result<()> {
    for (what, dir) in builder {
        // (the tools folder may hold the game folder: a source tree's build-linux/game; only a game folder holding
        // the tools would take them in its clean-up)
        let clash = if *what == TOOLS { within(dir, out) } else { overlap(out, dir) };
        if clash {
            return Err(Error::player(
                "game_folder_overlaps",
                format!(
                    "The game folder {} and the builder's {what} {} are one inside the other. Choose another game folder.",
                    out.display(),
                    dir.display()
                ),
            ));
        }
    }
    let theirs: Vec<&Path> = match game {
        GameSource::Nup(n) => vec![n.as_path()],
        GameSource::Folder { romfs, exefs } => vec![romfs.as_path(), exefs.as_path()],
    };
    for (what, ours) in [("game folder", out), ("work folder", work)] {
        for t in &theirs {
            let clash = match game {
                GameSource::Nup(_) => compared(t).starts_with(compared(ours)),
                GameSource::Folder { .. } => overlap(ours, t),
            };
            if clash {
                return Err(Error::player(
                    "game_files_overlap",
                    format!(
                        "Your game files {} and the {what} {} are one inside the other: the build would write over \
                         them. Choose a {what} apart from your game files.",
                        t.display(),
                        ours.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

// ---- installing

/// Where an earlier builder kept the installed note, apart from the record: read when the record has no `files`
/// section ([`RECORD_FILE`]), and gone once the record is next written (its `files` then hold it).
pub const INSTALLED_NOTE: &str = ".longshot_installed.json";

/// The installed note's key in the record ([`RECORD_FILE`]): what the last finish copied in, each file's hash and stat,
/// what `verify` checks the folder against. The record and the note are one file, written whole together.
pub const FILES_KEY: &str = "files";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Installed {
    /// The file's SHA-256, lower-case hex, as it was copied in ([`TREE`] for a folder's entry). A builder before
    /// wrote a SHA-1 under `sha1`: read here, it matches no file, so the next finish copies each once again, and
    /// `verify` says it can't check it until then.
    #[serde(alias = "sha1")]
    sha256: String,
    stat: (u64, u128, u64),
}

/// Whether `h` is a SHA-256's hex (not an older note's SHA-1, nor [`TREE`]).
fn is_sha256(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What says a file in the game folder is still the one the finish put there: nothing but a write that keeps its size
/// and time, in place, changes all three.
fn stat_of(path: &Path) -> Option<(u64, u128, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    #[cfg(unix)]
    let id = std::os::unix::fs::MetadataExt::ino(&m);
    #[cfg(not(unix))]
    let id = 0;
    Some((m.len(), t, id))
}

fn rel_name(out: &Path, dst: &Path) -> String {
    dst.strip_prefix(out).unwrap_or(dst).to_string_lossy().replace('\\', "/")
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// The installed note's entry for a folder the builder made whole (`romfs/`, a dump's linked or copied in): a name
/// ending in `/`, no file's hash or stat.
const TREE: &str = "tree";

/// The installed note: the record's `files` ([`FILES_KEY`]), else (a folder an earlier builder made) its own file
/// ([`INSTALLED_NOTE`]); each name checked to be a file inside the game folder ([`fsx::relative_ok`]; a folder's entry
/// ends in `/`): a note written by hand or copied in names nothing outside it for a sweep or a removal to touch.
fn read_note(out: &Path) -> BTreeMap<String, Installed> {
    let note: BTreeMap<String, Installed> = match record_object(out).remove(FILES_KEY) {
        Some(files) => serde_json::from_value(files).unwrap_or_default(),
        None => std::fs::read(out.join(INSTALLED_NOTE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default(),
    };
    note.into_iter().filter(|(rel, _)| fsx::relative_ok(rel.strip_suffix('/').unwrap_or(rel))).collect()
}

/// The record's file as a JSON object, every key in it (the current name's, else an earlier name's: the record and
/// its note are written back under the current one); empty when there is none or it is not an object.
fn record_object(out: &Path) -> serde_json::Map<String, serde_json::Value> {
    RECORD_FILES
        .iter()
        .find_map(|n| std::fs::read(out.join(n)).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| match v {
            serde_json::Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

/// The record's object `m` written whole as [`RECORD_FILE`] (beside it, then renamed over it: a reader sees the record
/// and its note of one write), then what it replaces removed: an earlier builder's separate note ([`INSTALLED_NOTE`])
/// and a record of an old name ([`OLD_RECORD_FILES`]: only one that is a record).
fn write_record_object(out: &Path, m: serde_json::Map<String, serde_json::Value>) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&serde_json::Value::Object(m))
        .map_err(|e| Error::format("the build record", e.to_string()))?;
    fsx::write_if_changed(&out.join(RECORD_FILE), &bytes)?;
    let _ = fsx::remove_file(&out.join(INSTALLED_NOTE));
    for old in OLD_RECORD_FILES {
        let path = out.join(old);
        if std::fs::read(&path).ok().and_then(|b| Record::parse(&b)).is_some() {
            fsx::remove_file(&path)?;
        }
    }
    Ok(())
}

/// The game folder's `romfs/`, linked or copied in from a dump by this build, noted as the builder's (best effort: a
/// note not written leaves the folder taken for the player's, kept, unless every file is a link of the dump's).
pub fn note_romfs_made(out: &Path) {
    let mut note = read_note(out);
    note.insert("romfs/".into(), Installed { sha256: TREE.into(), stat: (0, 0, 0) });
    let _ = write_note(out, &note);
}

/// Whether the game folder's `romfs/` is the builder's to remove: the installed note says a build made it, or (a
/// build before the note said so) every file in it is a hard link of the file of the same place in the dump the
/// folder's record says it was last built from. Never a folder of the player's own.
pub fn romfs_made_here(out: &Path) -> bool {
    if read_note(out).get("romfs/").is_some_and(|i| i.sha256 == TREE) {
        return true;
    }
    let Ok(Record { game: GameSource::Folder { romfs: from, .. }, .. }) = Record::read(out) else { return false };
    let dir = out.join("romfs");
    let Ok(files) = fsx::files_under(&dir) else { return false };
    !files.is_empty() && files.iter().all(|rel| fsx::same_file(&dir.join(rel), &from.join(rel)))
}

/// What old builders left in a game folder that nothing reads now, by name (make_game's: the game in `bin/`, its
/// exefs and romfs copies): `bin/`'s files of those names (and `bin/` once empty), `.longshot_exefs/`, and every
/// `.longshot_romfs*` but the index's note.
const BIN_FILES: [&str; 12] = [
    "longshot",
    "longshot.exe",
    "longshot_vk.so",
    "longshot_vk.dll",
    "libSDL2-2.0.so.0",
    "SDL2.dll",
    "SDL3.dll",
    "renhdrum.so",
    "renhdrum.dll",
    "longshot.pdb",
    "longshot_vk.pdb",
    "image.bin",
];

/// After a swap that went in: [`index_removals`], then what old builders left ([`sweep_old_builders`]): every
/// removal best effort, what it removed by name.
pub fn after_swap(out: &Path, remove: &[String], romfs_dir: crate::stages::RomfsDir) -> Vec<String> {
    let mut gone = index_removals(out, remove, romfs_dir);
    gone.extend(sweep_old_builders(out));
    gone
}

/// What the romfs's index takes out of the game folder: the builder's own files this index has none of (`remove`,
/// by name), and the `romfs/` folder when `romfs_dir` is [`RomfsDir::Remove`](crate::stages::RomfsDir::Remove) (the
/// stale dump's folder the game would read before the .nup). All `longshot-builder index` removes: it installs no
/// game, so an old builder's game in the folder stays the one there is to play.
pub fn index_removals(out: &Path, remove: &[String], romfs_dir: crate::stages::RomfsDir) -> Vec<String> {
    let mut gone = Vec::new();
    for rel in remove.iter().filter(|r| fsx::relative_ok(r)) {
        let p = out.join(rel);
        if p.is_file() && std::fs::remove_file(&p).is_ok() {
            gone.push(rel.clone());
        }
    }
    if romfs_dir == crate::stages::RomfsDir::Remove && fsx::remove_tree(&out.join("romfs")).is_ok() {
        let mut note = read_note(out);
        if note.remove("romfs/").is_some() {
            let _ = write_note(out, &note);
        }
        gone.push("romfs/".into());
    }
    gone
}

/// What old builders left that nothing reads now ([`BIN_FILES`] in `bin/`, `bin/` once empty, `.longshot_exefs/`,
/// the old `.longshot_romfs*`), removed: only after a build installed the game that replaces them.
pub fn sweep_old_builders(out: &Path) -> Vec<String> {
    let mut gone = Vec::new();
    let bin = out.join("bin");
    if bin.is_dir() && !bin.is_symlink() {
        for name in BIN_FILES {
            if bin.join(name).is_file() && std::fs::remove_file(bin.join(name)).is_ok() {
                gone.push(format!("bin/{name}"));
            }
        }
        if std::fs::remove_dir(&bin).is_ok() {
            gone.push("bin/".into());
        }
    }
    for e in std::fs::read_dir(out).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old =
            name == ".longshot_exefs" || (name.starts_with(".longshot_romfs") && name != game_files::romfs::INDEX_NOTE);
        if !old {
            continue;
        }
        let p = e.path();
        let done =
            if p.is_dir() && !p.is_symlink() { fsx::remove_tree(&p).is_ok() } else { fsx::remove_file(&p).is_ok() };
        if done {
            gone.push(name);
        }
    }
    gone
}

/// The installed note written as the record's `files`, the rest of the record as it is, in one write
/// ([`write_record_object`]).
fn write_note(out: &Path, note: &BTreeMap<String, Installed>) -> std::result::Result<(), String> {
    let mut m = record_object(out);
    m.insert(FILES_KEY.into(), serde_json::to_value(note).map_err(|e| e.to_string())?);
    write_record_object(out, m).map_err(|e| e.to_string())
}

/// Whether the file at `dst` in the game folder is one the builder put there and nobody changed since (the installed
/// note's stat): such a file is the builder's to replace or remove; any other is the player's own, left alone.
pub fn ours(out: &Path, dst: &Path) -> bool {
    owned(&read_note(out), out, dst)
}

fn owned(note: &BTreeMap<String, Installed>, out: &Path, dst: &Path) -> bool {
    note.get(&rel_name(out, dst)).is_some_and(|h| Some(h.stat) == stat_of(dst))
}

/// [`ours`] of each of `dsts`, the note read once.
pub fn ours_of(out: &Path, dsts: impl Iterator<Item = PathBuf>) -> Vec<(PathBuf, bool)> {
    let note = read_note(out);
    dsts.map(|d| {
        let o = owned(&note, out, &d);
        (d, o)
    })
    .collect()
}

/// The names the installed note holds (what the builder put in the game folder).
pub fn installed(out: &Path) -> Vec<String> {
    read_note(out).into_keys().collect()
}

/// A file the installed note lists, as `verify` checks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noted {
    /// Its name in the game folder (`/` between folders).
    pub rel: String,
    /// Its size as it was put there.
    pub size: u64,
    /// Its SHA-256 as it was put there; `None` in a note of a builder before (a SHA-1), which nothing checks.
    pub sha256: Option<String>,
}

/// The files the installed note lists, and the folders it lists whole (`romfs/`, a dump's: no file's hash).
pub fn noted(out: &Path) -> (Vec<Noted>, Vec<String>) {
    let (mut files, mut trees) = (Vec::new(), Vec::new());
    for (rel, i) in read_note(out) {
        if rel.ends_with('/') {
            trees.push(rel);
        } else {
            let sha256 = is_sha256(&i.sha256).then_some(i.sha256);
            files.push(Noted { rel, size: i.stat.0, sha256 });
        }
    }
    (files, trees)
}

/// The files the finish writes itself after the swap, noted by [`note_written`].
pub const WRITTEN: [&str; 2] = ["exefs_patches.txt", "longshot_version.txt"];

/// Files the finish wrote itself after the swap ([`WRITTEN`]), noted as the builder's with their hash, so `verify`
/// checks them too; one that is not there taken out of the note. Best effort, as the note is: why it could not be
/// written, if it could not.
pub fn note_written(out: &Path, rels: &[&str]) -> std::result::Result<(), String> {
    let mut note = read_note(out);
    for rel in rels {
        let path = out.join(rel);
        match (sha256_file(&path), stat_of(&path)) {
            (Some(sha256), Some(stat)) => note.insert(rel.to_string(), Installed { sha256, stat }),
            _ => note.remove(*rel),
        };
    }
    write_note(out, &note)
}

/// The file `rel` of the game folder put back from `src` (a copy of the bytes the note lists for it), whole: copied
/// in beside it as `<rel>.new`, then renamed over it, and its stat noted so the next finish keeps it. The caller holds
/// the game folder ([`build_core::lock::GameLock`]) and has checked `src`'s bytes.
pub fn put_back(out: &Path, rel: &str, src: &Path) -> Result<()> {
    if !fsx::relative_ok(rel) {
        return Err(Error::format(rel, "not a name inside the game folder"));
    }
    let dst = out.join(rel);
    let new = with_suffix(&dst, ".new");
    let _ = std::fs::remove_file(&new);
    let copied = new.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
        let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(&new)?;
        let mut from = std::fs::File::open(src)?;
        std::io::copy(&mut from, &mut to)?;
        to.set_permissions(writable(from.metadata()?.permissions()))
    });
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&new);
        if build_core::is_disk_full(&e) {
            return Err(build_core::disk_full(&new));
        }
        return Err(Error::io(&new, e));
    }
    if let Err(e) = fsx::rename(&new, &dst) {
        let _ = std::fs::remove_file(&new);
        return Err(locked(&dst, &e));
    }
    let mut note = read_note(out);
    if let (Some(i), Some(stat)) = (note.get_mut(rel), stat_of(&dst)) {
        i.stat = stat;
        write_note(out, &note).map_err(|e| Error::format(out.join(RECORD_FILE).display().to_string(), e))?;
    }
    Ok(())
}

/// What a swap that stopped part way (the builder stopped, the PC lost power) left beside `dst`, made whole: a
/// `<dst>.old` whose `dst` is missing put back (the swap had moved it aside and not put the new one in yet); any other
/// `.old` removed (on Windows, one a running game holds: by the next build, or at the PC's restart for an administrator,
/// the only one Windows takes that from); a
/// `<dst>.new` (staged, never swapped in) removed.
fn sweep(dst: &Path) {
    let (old, new) = (with_suffix(dst, ".old"), with_suffix(dst, ".new"));
    if old.symlink_metadata().is_ok() {
        if dst.symlink_metadata().is_err() {
            let _ = fsx::rename(&old, dst);
        } else {
            fsx::remove_or_later(&old);
        }
    }
    let _ = std::fs::remove_file(&new);
}

/// What [`install`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallCount {
    /// Files copied in.
    pub copied: usize,
    /// Files already there as they would be.
    pub kept: usize,
    /// The files of `remove` the builder had put there, removed (by name).
    pub removed: Vec<String>,
    /// Why the installed note could not be written, if it could not (the next finish copies every file again, and
    /// takes no vendor file there for its own).
    pub note_unwritten: Option<String>,
}

fn sha256_file(path: &Path) -> Option<String> {
    crate::sha256_file(path).ok().map(|(sha, _)| sha)
}

/// The SHA-256 of a file, lower-case hex.
fn sha256_of(path: &Path) -> Result<String> {
    use build_core::IoContext;
    use sha2::Digest;
    let mut f = std::fs::File::open(path).at(path)?;
    let mut h = sha2::Sha256::new();
    std::io::copy(&mut f, &mut h).at(path)?;
    Ok(build_core::key::to_hex(&h.finalize()))
}

fn locked(dst: &Path, e: &std::io::Error) -> Error {
    Error::player(
        "game_running",
        format!(
            "Could not replace or remove {} ({e}). Close any running copy of Longshot from this folder, then build \
             again. The game folder is as it was before this build.",
            dst.display()
        ),
    )
}

/// `files` (source, destination in `out`) put in the game folder and the builder's own files of `remove` taken out,
/// all whole: the changed files staged as `<dst>.new`, then swapped in together, each old file (and each one to
/// remove) moved aside first (`<dst>.old`) and put back if a later step fails, so the folder holds the old build or
/// the new one; the note of what was installed written after. First, what a swap stopped part way left is made
/// whole ([`sweep`]). A file of `remove` that is not the builder's (the player's own) stays. A destination of `pins`
/// (a vendor's file) is checked on its staged copy, the bytes swapped in, against its SHA-256.
pub fn install(
    out: &Path,
    files: &[(PathBuf, PathBuf)],
    pins: &[(PathBuf, &str)],
    remove: &[PathBuf],
) -> Result<InstallCount> {
    let mut note = read_note(out);
    for rel in note.keys().filter(|r| !r.ends_with('/')) {
        sweep(&out.join(rel));
    }
    for dst in files.iter().map(|f| &f.1).chain(remove) {
        sweep(dst);
    }
    let mut staged: Vec<(PathBuf, Option<PathBuf>, String)> = Vec::new();
    let mut count = InstallCount::default();
    let unstage = |staged: &[(PathBuf, Option<PathBuf>, String)]| {
        for n in staged.iter().filter_map(|s| s.1.as_ref()) {
            let _ = std::fs::remove_file(n);
        }
    };
    for (src, dst) in files {
        // (SHA-256, which the CPU's own instructions make faster than sha1_smol's SHA-1: `verify` checks by it)
        let sha = sha256_of(src)?;
        let had = note.get(&rel_name(out, dst));
        // (the destination itself is never read: on a cold cache that read took longer than the copy)
        if had.is_some_and(|h| h.sha256 == sha && Some(h.stat) == stat_of(dst)) {
            count.kept += 1;
            continue;
        }
        let new = with_suffix(dst, ".new");
        // (a file of its own: create_new refuses whatever is at that name, a link put there among them)
        let copied = new.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| {
            let mut to = std::fs::OpenOptions::new().write(true).create_new(true).open(&new)?;
            let mut from = std::fs::File::open(src)?;
            std::io::copy(&mut from, &mut to)?;
            // (the source's permissions as fs::copy kept them, the game's executable executable, but writable by
            // the player: a read-only source, a Nix store or a packaged install, gave read-only files)
            to.set_permissions(writable(from.metadata()?.permissions()))
        });
        if let Err(e) = copied {
            unstage(&staged);
            let _ = std::fs::remove_file(&new);
            if build_core::is_disk_full(&e) {
                return Err(build_core::disk_full(&new));
            }
            return Err(Error::player(
                "stage_failed",
                format!(
                    "Could not put the new build at {} ({e}). Check that the game folder is writable and has room, then \
                     build again. Nothing of the game there was replaced.",
                    new.display()
                ),
            ));
        }
        if let Some((_, want)) = pins.iter().find(|(p, _)| p == dst) {
            let got = sha256_file(&new);
            if got.as_deref() != Some(*want) {
                unstage(&staged);
                let _ = std::fs::remove_file(&new);
                return Err(Error::format(
                    new.display().to_string(),
                    format!("a vendor's file copied in as {}, not its pinned SHA-256 {want}", got.unwrap_or_default()),
                ));
            }
        }
        staged.push((dst.clone(), Some(new), sha));
    }
    for dst in remove.iter().filter(|d| owned(&note, out, d)) {
        staged.push((dst.clone(), None, String::new()));
    }
    // (each step: the old file aside, the new one in (none: a removal); undone in reverse on the first that fails)
    let mut done: Vec<(&PathBuf, Option<PathBuf>, bool)> = Vec::new();
    let mut failed = None;
    for (dst, new, _) in &staged {
        let old = with_suffix(dst, ".old");
        let aside = match dst.symlink_metadata() {
            Ok(_) => match fsx::rename(dst, &old) {
                Ok(()) => Some(old),
                Err(e) => {
                    failed = Some((dst, e));
                    break;
                }
            },
            Err(_) => None,
        };
        let Some(new) = new else {
            done.push((dst, aside, false));
            continue;
        };
        // (Linux: a running copy keeps its own file; Windows: a running exe can be moved aside, not written over)
        match fsx::rename(new, dst) {
            Ok(()) => done.push((dst, aside, true)),
            Err(e) => {
                if let Some(old) = aside {
                    let _ = fsx::rename(&old, dst);
                }
                failed = Some((dst, e));
                break;
            }
        }
    }
    if let Some((dst, e)) = failed {
        for (d, aside, placed) in done.into_iter().rev() {
            if placed {
                let _ = std::fs::remove_file(d);
            }
            if let Some(old) = aside {
                let _ = fsx::rename(&old, d);
            }
        }
        unstage(&staged);
        return Err(locked(dst, &e));
    }
    for (_, aside, _) in &done {
        if let Some(old) = aside {
            fsx::remove_or_later(old); // (a running exe's old file: gone at the next build's sweep)
        }
    }
    for (dst, new, sha256) in staged {
        let rel = rel_name(out, &dst);
        if new.is_none() {
            note.remove(&rel);
            if let Some(dir) = dst.parent().filter(|d| *d != out) {
                let _ = std::fs::remove_dir(dir); // (its folder too, once empty: ngx/)
            }
            count.removed.push(rel);
            continue;
        }
        if let Some(stat) = stat_of(&dst) {
            note.insert(rel, Installed { sha256, stat });
        }
        count.copied += 1;
    }
    count.note_unwritten = write_note(out, &note).err();
    Ok(count)
}

/// `p`, writable by its owner (and no one more).
fn writable(mut p: std::fs::Permissions) -> std::fs::Permissions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.set_mode(p.mode() | 0o200);
    }
    #[cfg(not(unix))]
    #[allow(clippy::permissions_set_readonly_false)] // (Windows: the read-only attribute, not a mode)
    p.set_readonly(false);
    p
}

// ---- the folder's own files

/// The language a game folder was built in: the code line of its language.txt.
pub fn folder_language(out: &Path) -> Option<&'static str> {
    let text = std::fs::read_to_string(out.join("language.txt")).ok()?;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .find_map(|l| LANGUAGES.iter().map(|x| x.0).find(|c| *c == l))
}

/// `language.txt`: the code the game reads (host/nn_applet.c writes the same text).
pub fn language_text(code: &str) -> String {
    let mut s = String::from(
        "# The game's language. Change the line below to one of the codes listed here, save, and start the game \
         again.\n\n",
    );
    s.push_str(code);
    s.push_str("\n\n");
    for (c, name) in LANGUAGES {
        s.push_str(&format!("# {c:<8} {name}\n"));
    }
    s
}

/// The game folder's `longshot_options.ini`, which the game's menu also writes.
pub fn options_path(out: &Path) -> PathBuf {
    out.join("longshot_options.ini")
}

const COMPILE_KEY: &str = "compile_shaders";

/// The Compile shaders choice the folder holds, if it holds one.
pub fn folder_compile_shaders(out: &Path) -> Option<CompileShaders> {
    let text = std::fs::read_to_string(options_path(out)).ok()?;
    text.lines().find_map(|l| {
        let (k, v) = l.trim().split_once('=')?;
        (k.trim() == COMPILE_KEY).then(|| if v.trim() == "1" { CompileShaders::All } else { CompileShaders::Most })
    })
}

/// `compile_shaders=<n>` into the folder's `longshot_options.ini`, its other lines as they are. A file with only this
/// line still makes the game's first start a first start (gpu/vk/vk_menu.cpp menu_init).
pub fn write_compile_shaders(out: &Path, level: CompileShaders) -> Result<()> {
    let path = options_path(out);
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> =
        old.lines().filter(|l| l.split('=').next().map(str::trim) != Some(COMPILE_KEY)).collect();
    let line = format!("{COMPILE_KEY}={}", level.number());
    lines.push(&line);
    fsx::write_if_changed(&path, format!("{}\n", lines.join("\n")).as_bytes()).map(|_| ())
}

/// `longshot_version.txt`: which release these tools are (`version=`) and their sources' fingerprint (`source=`, the
/// game's first log line's: the crash report calls a build a release only when the two match a release's), and where
/// the game sends what the player agreed to send (the tree's addresses).
pub fn version_text(
    release: &str,
    describe: &str,
    builder: Option<&str>,
    addresses: &BTreeMap<String, String>,
) -> String {
    let mut s = format!("version={release}\nsource={describe}\n");
    if let Some(exe) = builder {
        s.push_str(&format!("builder={exe}\n"));
    }
    for key in ["crash_reports", "perf_reports"] {
        if let Some(v) = addresses.get(key).filter(|v| v.starts_with("https://")) {
            s.push_str(&format!("{key}={v}\n"));
        }
    }
    s
}

/// The tree's list of what the game can do (`game/features.txt`: one name a line, `#` comments), as names: what the
/// build records ([`Record::features`]) and `longshot-builder info` says as the game's capabilities.
pub fn features(tree_list: &str) -> Vec<String> {
    tree_list
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
        .map(str::to_string)
        .collect()
}

/// The `[addresses]` of an ini file (the tree's addresses.ini): `key = value` lines.
pub fn read_addresses(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.starts_with('['))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

// ---- the build record

/// The record a build leaves in the game folder (`longshot_build.json`): what `longshot-builder update` builds it again
/// from.
pub const RECORD_FILE: &str = "longshot_build.json";
/// The record's names in a game folder an earlier development builder made (`longshot_builder.json`, then for a while
/// `longshot.json`, a name the game folder's record no longer has): read in this order
/// when [`RECORD_FILE`] is missing, and removed when the record is next written (only one that is a record).
pub const OLD_RECORD_FILES: [&str; 2] = ["longshot_builder.json", "longshot.json"];
/// Every name the record of a game folder can have, the current one first.
pub const RECORD_FILES: [&str; 3] = [RECORD_FILE, OLD_RECORD_FILES[0], OLD_RECORD_FILES[1]];
const RECORD_FORMAT: u32 = 1;

/// What a game folder was built from and with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The record's format.
    pub format: u32,
    /// The builder's release that built it.
    pub builder: String,
    /// The player's copy.
    pub game: GameSource,
    /// The upscalers built in.
    pub upscalers: Vec<Upscaler>,
    /// What play's fast check compares with now (crate::play::check): written by a build or an update for a profile
    /// (play, `build`/`update --profile`); none from one without, which the next play updates again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stamp: Option<Stamp>,
    /// What the game built here can do that a launcher may ask of it, from the sources it was built from
    /// (`game/features.txt`, [`features`]): `longshot-builder info`'s game capabilities. Empty in a record of an
    /// earlier builder (whose `longshot_version.txt` had a `features=` line, which `info` reads then).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// The stamp of a game folder's last build for a profile (the record's `stamp`): each part a string compared whole.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// The builder that made it: its release and source, or a developer's build's own (longshot-builder's main.rs).
    pub builder: String,
    /// The key of what of the profile decides the build: its copy and upscalers (crate::play::Profile::key).
    pub profile: String,
    /// The player's copy's size, time and file (crate::play::game_stamp).
    pub game: String,
    /// The mods that need a build, when the Mod API gives their stamp (crate::play::mods_stamp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mods: Option<String>,
    /// The ExeFS patches the profile's enabled manifest mods have (crate::play::patches_stamp): empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub patches: String,
}

impl Record {
    /// A record for this build.
    pub fn new(builder: &str, game: &GameSource, upscalers: &[Upscaler]) -> Record {
        Record {
            format: RECORD_FORMAT,
            builder: builder.to_string(),
            game: game.clone(),
            upscalers: upscalers.to_vec(),
            stamp: None,
            features: Vec::new(),
        }
    }

    /// The record of game folder `out`, or the player told it was not built by this builder.
    pub fn read(out: &Path) -> Result<Record> {
        let not_ours = || {
            Error::player(
                "not_built_here",
                format!(
                    "The folder {} was not built by this builder (it has no {RECORD_FILE}). Choose your game files and \
                     build into it once; updates after that work by themselves.",
                    out.display()
                ),
            )
        };
        // (the first name there is the record: an old one only when the current one is missing)
        let bytes = RECORD_FILES.iter().find_map(|n| std::fs::read(out.join(n)).ok()).ok_or_else(not_ours)?;
        Self::parse(&bytes).ok_or_else(not_ours)
    }

    /// `bytes` as a record of this builder's format.
    fn parse(bytes: &[u8]) -> Option<Record> {
        serde_json::from_slice::<Record>(bytes).ok().filter(|r| r.format == RECORD_FORMAT)
    }

    /// The record written into `out`, with the installed note the folder has ([`FILES_KEY`]: the record's own, else
    /// an earlier builder's separate file, merged in now) in the same write; one of an old name ([`OLD_RECORD_FILES`])
    /// and the separate note removed once it is (only a record, never another file of that name).
    pub fn write(&self, out: &Path) -> Result<()> {
        let mut m = match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(m)) => m,
            Ok(_) => return Err(Error::format("the build record", "not an object")),
            Err(e) => return Err(Error::format("the build record", e.to_string())),
        };
        let note = read_note(out);
        if !note.is_empty() {
            m.insert(
                FILES_KEY.into(),
                serde_json::to_value(&note).map_err(|e| Error::format("the build record", e.to_string()))?,
            );
        }
        write_record_object(out, m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::scratch;

    #[test]
    #[cfg(unix)]
    fn a_folder_not_made_yet_under_a_link_to_the_dump_is_inside_the_dump() {
        let d = scratch("compared");
        std::fs::create_dir_all(d.join("dump/romfs")).unwrap();
        std::os::unix::fs::symlink(d.join("dump"), d.join("link")).unwrap();
        let game = GameSource::Folder { romfs: d.join("dump/romfs"), exefs: d.join("dump/exefs") };
        let out = d.join("link/romfs/Longshot"); // (not there: the build would make it, in the dump)
        let e = check_places(&out, &d.join("work"), &[], &game).unwrap_err();
        assert!(matches!(e, Error::Player { code: "game_files_overlap", .. }), "{e}");
        assert_eq!(compared(&d.join("link/a/../b")), compared(&d.join("dump")).join("b"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_install_copies_what_changed_and_keeps_what_did_not() {
        let d = scratch("install");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a"), b"one").unwrap();
        std::fs::write(src.join("b"), b"two").unwrap();
        let files = vec![(src.join("a"), out.join("a")), (src.join("b"), out.join("bin/b"))];
        let count = |c: InstallCount| (c.copied, c.kept);
        assert_eq!(count(install(&out, &files, &[], &[]).unwrap()), (2, 0));
        assert_eq!(std::fs::read(out.join("bin/b")).unwrap(), b"two");
        assert_eq!(count(install(&out, &files, &[], &[]).unwrap()), (0, 2));
        std::fs::write(src.join("a"), b"three").unwrap();
        assert_eq!(count(install(&out, &files, &[], &[]).unwrap()), (1, 1));
        assert_eq!(std::fs::read(out.join("a")).unwrap(), b"three");
        // (a file changed in the folder since is copied again)
        std::fs::write(out.join("bin/b"), b"twO").unwrap();
        assert_eq!(install(&out, &files, &[], &[]).unwrap().copied, 1);
        assert_eq!(std::fs::read(out.join("bin/b")).unwrap(), b"two");
        assert!(!out.join("a.new").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_swap_that_fails_half_way_leaves_the_old_build() {
        let d = scratch("rollback");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        for (n, t) in [("a", "old a"), ("b", "old b")] {
            std::fs::write(src.join(n), t).unwrap();
        }
        let files = vec![(src.join("a"), out.join("a")), (src.join("b"), out.join("b"))];
        install(&out, &files, &[], &[]).unwrap();
        std::fs::write(src.join("a"), "new a").unwrap();
        std::fs::write(src.join("b"), "new b").unwrap();
        // (b cannot be moved aside, as a file held open on Windows: a folder in its place, and one where it would go)
        std::fs::remove_file(out.join("b")).unwrap();
        std::fs::create_dir_all(out.join("b/held")).unwrap();
        std::fs::create_dir_all(out.join("b.old/held")).unwrap();
        assert!(matches!(install(&out, &files, &[], &[]), Err(Error::Player { code: "game_running", .. })));
        assert_eq!(std::fs::read_to_string(out.join("a")).unwrap(), "old a", "the first swap was undone");
        let left: Vec<PathBuf> = fsx::files_under(&out).unwrap();
        assert!(
            left.iter().all(|f| !f.to_string_lossy().ends_with(".new") && !f.to_string_lossy().ends_with(".old")),
            "{left:?}"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn only_what_the_builder_put_there_is_its_to_replace_or_remove() {
        let d = scratch("ours");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("lib.dll"), "pinned 1").unwrap();
        let ngx = out.join("ngx/x.dll");
        install(&out, &[(src.join("lib.dll"), out.join("lib.dll")), (src.join("lib.dll"), ngx.clone())], &[], &[])
            .unwrap();
        assert!(ours(&out, &out.join("lib.dll")));
        std::fs::write(out.join("theirs.dll"), "the player's").unwrap();
        assert!(!ours(&out, &out.join("theirs.dll")));
        std::fs::write(out.join("lib.dll"), "the player's newer").unwrap();
        assert!(!ours(&out, &out.join("lib.dll")), "changed since: the player's");
        let gone = install(&out, &[], &[], &[out.join("lib.dll"), out.join("theirs.dll"), ngx.clone()]).unwrap();
        assert_eq!(gone.removed, ["ngx/x.dll"]);
        assert!(out.join("lib.dll").is_file() && out.join("theirs.dll").is_file() && !out.join("ngx").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_removal_is_undone_with_the_rest_when_a_swap_fails() {
        let d = scratch("remove-undone");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("dll"), "a vendor's").unwrap();
        std::fs::write(src.join("exe"), "old exe").unwrap();
        install(&out, &[(src.join("dll"), out.join("ngx/x.dll")), (src.join("exe"), out.join("exe"))], &[], &[])
            .unwrap();
        // (DLSS no longer asked for, and the new exe cannot go in: the DLL is back as it was)
        std::fs::write(src.join("exe"), "new exe").unwrap();
        std::fs::remove_file(out.join("exe")).unwrap();
        std::fs::create_dir_all(out.join("exe/held")).unwrap();
        std::fs::create_dir_all(out.join("exe.old/held")).unwrap();
        let e = install(&out, &[(src.join("exe"), out.join("exe"))], &[], &[out.join("ngx/x.dll")]);
        assert!(matches!(e, Err(Error::Player { code: "game_running", .. })), "{e:?}");
        assert_eq!(std::fs::read_to_string(out.join("ngx/x.dll")).unwrap(), "a vendor's");
        assert!(ours(&out, &out.join("ngx/x.dll")));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn what_a_stopped_swap_left_is_made_whole_first() {
        let d = scratch("sweep");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        for (n, t) in [("a", "a1"), ("b", "b1")] {
            std::fs::write(src.join(n), t).unwrap();
        }
        let files = vec![(src.join("a"), out.join("a")), (src.join("b"), out.join("b"))];
        install(&out, &files, &[], &[]).unwrap();
        // (stopped between moving b aside and putting its new one in; a's new one staged and never swapped)
        std::fs::rename(out.join("b"), out.join("b.old")).unwrap();
        std::fs::write(out.join("a.new"), "a2").unwrap();
        let c = install(&out, &files, &[], &[]).unwrap();
        assert_eq!((c.copied, c.kept), (0, 2), "b is back as it was");
        assert_eq!(std::fs::read_to_string(out.join("b")).unwrap(), "b1");
        assert!(!out.join("a.new").exists() && !out.join("b.old").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_record_of_an_earlier_builder_with_a_user_folder_and_a_launcher_still_reads() {
        let rec = Record::new("v1", &GameSource::Nup("/g/x.nup".into()), &[Upscaler::Fsr4]);
        let mut json = serde_json::to_value(&rec).unwrap();
        assert!(json.get("user").is_none() && json.get("launcher").is_none(), "written without them");
        json["user"] = "/home/p/user".into();
        json["launcher"] = serde_json::json!({"kind": "recomplauncher", "program": "/r/R"});
        assert_eq!(
            Record::parse(&serde_json::to_vec(&json).unwrap()),
            Some(rec),
            "the keys it no longer has are ignored"
        );
    }

    #[test]
    fn a_record_of_an_old_name_is_read_and_written_again_under_the_new_one() {
        assert_eq!(RECORD_FILE, "longshot_build.json");
        let d = scratch("record-old-name");
        let out = d.join("out");
        let rec = Record::new("v1", &GameSource::Nup("/g/x.nup".into()), &[]);
        for old in OLD_RECORD_FILES {
            let _ = std::fs::remove_dir_all(&out);
            std::fs::create_dir_all(&out).unwrap();
            assert_eq!(Record::read(&out).unwrap_err().code(), "not_built_here");
            // (a folder an earlier builder made: its record of an old name is the folder's record)
            std::fs::write(out.join(old), serde_json::to_vec(&rec).unwrap()).unwrap();
            assert_eq!(Record::read(&out).unwrap(), rec, "{old}");
            // (written again, as the swap does: under the new name alone)
            rec.write(&out).unwrap();
            assert!(out.join(RECORD_FILE).is_file() && !out.join(old).exists(), "{old}");
            assert_eq!(Record::read(&out).unwrap(), rec);
            // (both there: the new one is read)
            let mut older = rec.clone();
            older.builder = "v0".into();
            std::fs::write(out.join(old), serde_json::to_vec(&older).unwrap()).unwrap();
            assert_eq!(Record::read(&out).unwrap().builder, "v1", "{old}");
        }
        // (a longshot.json that is no record (someone else's file): not read as one, and never removed)
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("longshot.json"), br#"{"v":1,"game":"/g/x.nup"}"#).unwrap();
        assert_eq!(Record::read(&out).unwrap_err().code(), "not_built_here");
        rec.write(&out).unwrap();
        assert!(out.join("longshot.json").is_file());
        assert_eq!(Record::read(&out).unwrap(), rec);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_romfs_folder_is_the_builder_s_only_when_it_made_it() {
        let d = scratch("romfs-ours");
        let (dump, out) = (d.join("dump/romfs"), d.join("out"));
        std::fs::create_dir_all(dump.join("a")).unwrap();
        std::fs::write(dump.join("a/f.bin"), b"game").unwrap();
        std::fs::create_dir_all(&out).unwrap();
        // (a player's own romfs folder, copied there by hand: not the builder's, and stays)
        fsx::copy_tree(&dump, &out.join("romfs")).unwrap();
        assert!(!romfs_made_here(&out));
        let keep = after_swap(&out, &[], crate::stages::RomfsDir::Theirs);
        assert!(keep.is_empty() && out.join("romfs/a/f.bin").is_file());
        // (an older build's links of the dump its record names: the builder's)
        let rec = Record::new("v1", &GameSource::Folder { romfs: dump.clone(), exefs: d.join("dump/exefs") }, &[]);
        rec.write(&out).unwrap();
        assert!(!romfs_made_here(&out), "copies, not links");
        fsx::remove_tree(&out.join("romfs")).unwrap();
        fsx::link_tree(&dump, &out.join("romfs")).unwrap();
        assert!(romfs_made_here(&out));
        // (this builder's: noted, links or not)
        std::fs::remove_file(out.join(RECORD_FILE)).unwrap();
        assert!(!romfs_made_here(&out));
        note_romfs_made(&out);
        assert!(romfs_made_here(&out));
        // (the note's folder entry is no file a sweep touches: an install keeps it)
        install(&out, &[], &[], &[]).unwrap();
        assert!(romfs_made_here(&out) && out.join("romfs/a/f.bin").is_file());
        assert_eq!(after_swap(&out, &[], crate::stages::RomfsDir::Remove), ["romfs/"]);
        assert!(!out.join("romfs").exists() && std::fs::read(dump.join("a/f.bin")).unwrap() == b"game");
        assert!(!read_note(&out).contains_key("romfs/"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_swap_takes_out_what_old_builds_left_and_this_one_has_none_of() {
        let d = scratch("after-swap");
        let out = d.join("out");
        std::fs::create_dir_all(out.join("bin")).unwrap();
        std::fs::create_dir_all(out.join(".longshot_exefs/main")).unwrap();
        for f in [
            "bin/longshot.exe",
            "bin/SDL2.dll",
            "longshot_damaged_files.txt",
            ".longshot_romfs.idx",
            ".longshot_romfs.json",
        ] {
            std::fs::write(out.join(f), b"old").unwrap();
        }
        let mut gone = after_swap(&out, &["longshot_damaged_files.txt".into(), "../escape".into()], Default::default());
        gone.sort();
        assert_eq!(
            gone,
            [
                ".longshot_exefs",
                ".longshot_romfs.idx",
                "bin/",
                "bin/SDL2.dll",
                "bin/longshot.exe",
                "longshot_damaged_files.txt"
            ]
        );
        assert!(out.join(".longshot_romfs.json").is_file(), "the index's own note stays");
        // (a player's file in bin/ keeps it)
        std::fs::create_dir_all(out.join("bin")).unwrap();
        std::fs::write(out.join("bin/mine.txt"), b"mine").unwrap();
        std::fs::write(out.join("bin/longshot"), b"old").unwrap();
        assert_eq!(after_swap(&out, &[], Default::default()), ["bin/longshot"]);
        assert!(out.join("bin/mine.txt").is_file());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_installed_note_is_the_record_s_files_and_an_earlier_builder_s_own_file_is_merged_in() {
        let d = scratch("note-merged");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(src.join("longshot"), b"game").unwrap();
        install(&out, &[(src.join("longshot"), out.join("longshot"))], &[], &[]).unwrap();
        let rec = Record::new("v1", &GameSource::Nup("/g/x.nup".into()), &[]);
        rec.write(&out).unwrap();
        let text = std::fs::read_to_string(out.join(RECORD_FILE)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(v[FILES_KEY]["longshot"]["sha256"].is_string(), "one file, the record and its note: {text}");
        assert_eq!(Record::read(&out).unwrap(), rec);
        assert!(!out.join(INSTALLED_NOTE).exists());
        assert!(ours(&out, &out.join("longshot")));
        // (the record written again keeps its note; a note written again keeps the record)
        rec.write(&out).unwrap();
        assert_eq!(installed(&out), ["longshot"]);
        note_written(&out, &["longshot"]).unwrap();
        assert_eq!(Record::read(&out).unwrap(), rec);
        // (a folder an earlier builder made: its note apart, read; merged at the next write, the file gone)
        let mut m: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&text).unwrap();
        let files = m.remove(FILES_KEY).unwrap();
        std::fs::write(out.join(RECORD_FILE), serde_json::to_vec(&m).unwrap()).unwrap();
        std::fs::write(out.join(INSTALLED_NOTE), serde_json::to_vec(&files).unwrap()).unwrap();
        assert_eq!(installed(&out), ["longshot"]);
        assert!(ours(&out, &out.join("longshot")));
        rec.write(&out).unwrap();
        assert!(!out.join(INSTALLED_NOTE).exists(), "merged into the record");
        assert_eq!(installed(&out), ["longshot"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_installed_note_naming_a_place_outside_the_folder_is_not_read() {
        let d = scratch("note-keys");
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(d.join("victim.old"), b"theirs").unwrap();
        let entry = r#"{"sha1":"x","stat":[0,0,0]}"#;
        let note = format!(r#"{{"../victim":{entry},"/etc/x":{entry},"a/./b":{entry},"ok/file":{entry}}}"#);
        std::fs::write(out.join(INSTALLED_NOTE), note).unwrap();
        assert_eq!(installed(&out), ["ok/file"]);
        install(&out, &[], &[], &[]).unwrap();
        assert!(d.join("victim.old").is_file(), "a sweep outside the game folder");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_vendor_file_whose_copy_is_not_its_pin_is_not_swapped_in() {
        let d = scratch("pins");
        let (src, out) = (d.join("src"), d.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("x.dll"), "bytes").unwrap();
        let files = [(src.join("x.dll"), out.join("x.dll"))];
        let wrong = [(out.join("x.dll"), "00")];
        assert!(install(&out, &files, &wrong, &[]).is_err());
        assert!(!out.join("x.dll").exists() && !out.join("x.dll.new").exists());
        let right = [(out.join("x.dll"), sha256_file(&src.join("x.dll")).unwrap())];
        let right: Vec<(PathBuf, &str)> = right.iter().map(|(p, h)| (p.clone(), h.as_str())).collect();
        assert_eq!(install(&out, &files, &right, &[]).unwrap().copied, 1);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_record_of_an_earlier_builder_with_licences_and_an_id_still_reads() {
        // (an earlier builder kept the licences accepted and an id in the record: ignored, the profile holds them now)
        let d = scratch("old-licences");
        let text = r#"{"format":1,"builder":"v1","game":{"nup":"/g/x.nup"},"upscalers":["dlss"],
            "accepted":[{"vendor":"nvidia","pin":"p"}],"id":"0123abcd"}"#;
        std::fs::write(d.join(RECORD_FILE), text).unwrap();
        let r = Record::read(&d).unwrap();
        assert_eq!((r.builder.as_str(), r.upscalers.as_slice()), ("v1", &[Upscaler::Dlss][..]));
        r.write(&d).unwrap();
        let again = std::fs::read_to_string(d.join(RECORD_FILE)).unwrap();
        assert!(!again.contains("accepted") && !again.contains("\"id\""), "written again without them: {again}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_folder_s_choices_read_back() {
        let d = scratch("choices");
        std::fs::write(d.join("language.txt"), language_text("de")).unwrap();
        assert_eq!(folder_language(&d), Some("de"));
        assert_eq!(folder_compile_shaders(&d), None);
        std::fs::write(options_path(&d), "fps=60\ncompile_shaders=0\n").unwrap();
        write_compile_shaders(&d, CompileShaders::All).unwrap();
        assert_eq!(std::fs::read_to_string(options_path(&d)).unwrap(), "fps=60\ncompile_shaders=1\n");
        assert_eq!(folder_compile_shaders(&d), Some(CompileShaders::All));
        let r = Record::new("v1", &GameSource::Nup("/g/x.nup".into()), &[Upscaler::Fsr4]);
        r.write(&d).unwrap();
        assert_eq!(Record::read(&d).unwrap(), r);
        assert!(matches!(Record::read(&d.join("none")), Err(Error::Player { code: "not_built_here", .. })));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_places_are_apart_from_the_builder_s_and_the_game_s() {
        let d = scratch("apart");
        let nup = GameSource::Nup(d.join("dl/g.nup"));
        let (game, work) = (d.join("game"), d.join("work"));
        assert!(check_places(&game, &work, &[("work folder", &work)], &nup).is_ok());
        assert!(check_places(&work.join("game"), &work, &[("work folder", &work)], &nup).is_err());
        assert!(check_places(&d, &work, &[("store", &d.join("store"))], &nup).is_err());
        // (a source tree's game folder is inside its tools; a game folder holding the tools is refused)
        assert!(check_places(&d.join("tree/build-linux/game"), &work, &[(TOOLS, &d.join("tree"))], &nup).is_ok());
        assert!(check_places(&d, &work, &[(TOOLS, &d.join("tree"))], &nup).is_err());
        // (a .nup beside the game folder is fine; inside the game folder or the work folder it is not)
        assert!(check_places(&d.join("dl/Longshot"), &work, &[], &nup).is_ok());
        assert!(check_places(&d.join("dl"), &work, &[], &nup).is_err());
        assert!(check_places(&game, &d.join("dl"), &[], &nup).is_err());
        // (a dump: either way round, the game folder or the work folder)
        let dump = d.join("dump");
        let folder = GameSource::Folder { romfs: dump.join("romfs"), exefs: dump.join("exefs") };
        assert!(check_places(&game, &work, &[], &folder).is_ok());
        for (out, w) in [(&dump, &work), (&dump.join("romfs/x"), &work), (&game, &dump.join("exefs")), (&d, &work)] {
            assert!(
                matches!(check_places(out, w, &[], &folder), Err(Error::Player { code: "game_files_overlap", .. })),
                "{} {}",
                out.display(),
                w.display()
            );
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_features_are_the_trees_names() {
        assert_eq!(features("# a comment\nstart_slot\n\n  other_one  # why\nBad Name\n"), ["start_slot", "other_one"]);
        assert!(features("# none yet\n").is_empty());
        // (the tree's own list: the profile is there, and the save folder switch it replaced is gone)
        let tree = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../game/features.txt")).unwrap();
        let names = features(&tree);
        assert!(!names.iter().any(|n| n == "save_dir") && names.iter().any(|n| n == "profile"), "{names:?}");
    }
}

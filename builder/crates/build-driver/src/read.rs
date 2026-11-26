//! The read stage over the game-files and image crates: the [`GameFiles`] the builder runs with.

use crate::options::GameSource;
use crate::stages::{Check, Disk, GameFiles, GameRead, Patches, RomfsDir, RomfsPlaced};
use build_core::report::Reporter;
use build_core::{Error, IoContext, Result, fsx};
use game_files::nup::{self, Nup, Part};
use game_files::{checks, disk, mod_layout, nso, patches, romfs, title};
use std::collections::BTreeSet;
use std::path::Path;

/// The read stage.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadStage;

/// The manifest mods of `profile` whose ExeFS patches a build takes (name, folder, on), in their order, as the game
/// finds and orders them (game/mods.c, docs/dev/reference/mods.md "Discovery and order"): a folder with `mod.json` one
/// to four levels below the profile's `mods/`, nothing below a mod searched; a folder named `-…` or `_…` off with
/// everything below it, `.…` skipped; each named by its path under `mods/` (`/`-separated). The order and the switches
/// are the profile's `mods.json` (`{"mods": [{"id": "local/<path>", "enabled": …}]}`): the mods it names first, in its
/// order, then the others, on, by path ignoring case. A `mods.json` that isn't such a list leaves every mod on.
pub fn manifest_mods(profile: Option<&Path>) -> Vec<patches::Mod> {
    let Some(profile) = profile else { return Vec::new() };
    let root = crate::play::mods(profile);
    let mut found: Vec<(String, std::path::PathBuf)> = Vec::new();
    find_manifest_mods(&root, "", 0, &mut found);
    let listed: Vec<(String, bool)> = std::fs::read(profile.join("mods.json"))
        .ok()
        .filter(|b| b.len() < 1 << 20)
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("mods").and_then(|m| m.as_array()).cloned())
        .map(|list| {
            list.iter()
                .filter_map(|e| {
                    let id = e.get("id")?.as_str()?.strip_prefix("local/")?.to_string();
                    Some((id, e.get("enabled").and_then(serde_json::Value::as_bool).unwrap_or(true)))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<patches::Mod> = Vec::new();
    for (id, on) in &listed {
        if let Some(i) = found.iter().position(|(n, _)| n == id) {
            let (name, dir) = found.remove(i);
            out.push((name, dir, *on));
        }
    }
    found.sort_by_key(|(n, _)| n.to_lowercase());
    out.extend(found.into_iter().map(|(n, d)| (n, d, true)));
    out
}

/// The manifest mods below `dir` (`name` its path under the mods folder), `depth` levels below it, into `out`.
fn find_manifest_mods(dir: &Path, name: &str, depth: usize, out: &mut Vec<(String, std::path::PathBuf)>) {
    if depth > 0 && dir.join("mod.json").is_file() {
        out.push((name.to_string(), dir.to_path_buf()));
        return;
    }
    if depth >= 4 {
        return;
    }
    let Ok(listing) = std::fs::read_dir(dir) else { return };
    let mut subs: Vec<_> = listing.flatten().filter(|e| e.path().is_dir()).collect();
    subs.sort_by_key(std::fs::DirEntry::file_name);
    for e in subs {
        let n = e.file_name().to_string_lossy().into_owned();
        if n.starts_with(['-', '_', '.']) {
            continue;
        }
        let sub = if name.is_empty() { n } else { format!("{name}/{n}") };
        find_manifest_mods(&e.path(), &sub, depth + 1, out);
    }
}

impl GameFiles for ReadStage {
    fn check(&self, game: &GameSource) -> Result<Check> {
        let (ok, build, text) = match game {
            GameSource::Nup(p) => checks::check(Some(p), None)?,
            GameSource::Folder { exefs, .. } => checks::check(None, Some(exefs))?,
        };
        Ok(Check { ok, build, text })
    }

    fn disk(&self, folder: &Path) -> Disk {
        Disk { free: disk::free_bytes(folder), spinning: disk::spinning(folder) }
    }

    fn check_writable(&self, out: &Path) -> Result<()> {
        checks::check_writable(out)
    }

    fn read_exefs(&self, game: &GameSource, exefs: &Path, staging: &Path, r: &Reporter) -> Result<GameRead> {
        read_exefs_of(game, exefs, staging, r, &[checks::SUPPORTED_BUILD])
    }

    fn patches(&self, exefs: &Path, profile: Option<&Path>, tables: &Path, r: &Reporter) -> Result<Patches> {
        let main = nso::Nso::load(&exefs.join("main"))?;
        // (the one mod format: a mod left in the emulators' layout in the profile's mods/ is said, never read)
        if let Some(mods) = profile.map(crate::play::mods) {
            let old = mod_layout::needing_import(&mods);
            if !old.is_empty() {
                r.warn("mod_needs_import", mod_layout::import_text(&mods, &old));
            }
        }
        let mut files = patches::collect(&main, &manifest_mods(profile), &mut |l| r.log(l))?;
        if !files.is_empty() {
            crate::clashes::refuse(&main, &sites::Tables::new(tables), &mut files)?;
        }
        let applied = files.iter().filter(|f| f.kind.applies()).count();
        let list = if files.is_empty() { String::new() } else { patches::list_text(&files) };
        Ok(Patches { files: patches::to_json(&files), list, applied })
    }

    fn image(&self, exefs: &Path, p: &Patches, hle: &BTreeSet<String>, image: &Path, r: &Reporter) -> Result<()> {
        let files = if p.files.is_empty() { Vec::new() } else { patches::from_json(&p.files)? };
        let opts = image::build::Options { hle: hle.clone(), ..Default::default() };
        let built = image::build::build_from_exefs(exefs, &files, &opts)?;
        built.write(image)?;
        r.log(format!(
            "image: {} modules, {} MB, {} hooks, {} unresolved imports",
            built.modules.len(),
            built.mem.len() >> 20,
            built.hooks.len(),
            built.unresolved.len()
        ));
        Ok(())
    }

    fn romfs(&self, game: &GameSource, out: &Path, staging: &Path, r: &Reporter) -> Result<RomfsPlaced> {
        let own = [romfs::INDEX_FILE, romfs::INDEX_NOTE, checks::DAMAGED_NOTE];
        match game {
            GameSource::Nup(p) => {
                let n = Nup::open(p)?;
                let t = n.pick_title(None)?;
                std::fs::create_dir_all(staging).at(staging)?;
                // (the game folder's own index, of this .nup, taken as the staged one's start: a work folder made
                // anew, or the first build that stages it, indexes nothing again)
                if !staging.join(romfs::INDEX_FILE).is_file() && out.join(romfs::INDEX_FILE).is_file() {
                    let _ = romfs::reuse(out, staging, &n);
                }
                let (done, kept) = romfs::index_or_keep(&n, t, staging, &mut |_| {})?;
                if kept {
                    r.log("romfs: the index is of this .nup, made by these rules: kept");
                }
                let index = romfs::Index::open(&staging.join(romfs::INDEX_FILE))?;
                checks::check_game_romfs(&checks::Romfs::Index(&index), &format!("Your .nup file {}", p.display()))?;
                let note = staging.join(checks::DAMAGED_NOTE);
                if done.stand_ins.is_empty() {
                    fsx::remove_file(&note)?;
                } else {
                    fsx::write_atomic(&note, checks::damaged_note(&done.stand_ins).as_bytes())?;
                    checks::unresolved(&done.stand_ins)?;
                }
                // (the game reads a romfs folder beside it before the index (host/hos_main.c): a stale dump's folder
                // would win over the .nup. One the builder made goes at the swap; a player's own stays, said)
                let romfs_dir = if !out.join("romfs").is_dir() {
                    RomfsDir::None
                } else if crate::folder::romfs_made_here(out) {
                    RomfsDir::Remove
                } else {
                    RomfsDir::Theirs
                };
                Ok(RomfsPlaced {
                    files: done.entries,
                    bytes: done.bytes,
                    stood_in: done.stand_ins.iter().map(|s| s.damaged.clone()).collect(),
                    romfs_dir,
                    // (an old damaged list the new build has none of)
                    remove: if done.stand_ins.is_empty() { vec![checks::DAMAGED_NOTE.into()] } else { vec![] },
                })
            }
            GameSource::Folder { romfs: from, .. } => {
                checks::check_romfs_folder(from)?;
                checks::refuse_dump_links(from)?;
                // (a folder's romfs: no index read over it, staged or in the game folder, where it goes at the swap)
                for f in own {
                    fsx::remove_file(&staging.join(f))?;
                }
                // (a romfs folder already in the game folder that the builder did not make is the player's own: the
                // links would write over its files, and the note would then take the whole folder for the builder's)
                let dir = out.join("romfs");
                let there = dir.symlink_metadata().is_ok();
                let empty = std::fs::read_dir(&dir).is_ok_and(|mut e| e.next().is_none()) && !dir.is_symlink();
                if there && !empty && !crate::folder::romfs_made_here(out) {
                    return Err(Error::player(
                        "romfs_folder_theirs",
                        format!(
                            "The game folder already has a romfs folder, {}, that the builder did not make: it is \
                             yours, and a build from your dump would write over it. Move it out of the game folder, \
                             or choose another game folder, then build again.",
                            dir.display()
                        ),
                    ));
                }
                // (hard links, no copy: where a link is refused, another drive, it is a copy of 26 GB. Linked now,
                // before the swap, as no 26 GB is staged: the player's own files, which the old build reads the same)
                // (noted before the first link when the builder makes it from nothing: a build stopped half way
                // leaves a folder still known for the builder's)
                let made = !there || empty;
                if made {
                    std::fs::create_dir_all(out).at(out)?;
                    crate::folder::note_romfs_made(out);
                }
                let how = fsx::link_tree(from, &dir)?;
                if how == fsx::Placed::Copied {
                    r.warn(
                        "romfs_copied",
                        "The romfs was copied into the game folder (it is on another drive): 26 GB.",
                    );
                }
                let files = fsx::files_under(from)?.len();
                Ok(RomfsPlaced { files, remove: own.iter().map(|f| f.to_string()).collect(), ..RomfsPlaced::default() })
            }
        }
    }
}

/// The game's name and icon as the read stage kept them: staged in `staging`, else (a dump's build, which reads no
/// title, or the first build that stages it) as the game folder `out` has them.
pub fn title_of(staging: &Path, out: &Path) -> (Option<String>, Option<Vec<u8>>) {
    match title::kept(staging) {
        (None, None) => title::kept(out),
        kept => kept,
    }
}

/// The read stage's exefs ([`GameFiles::read_exefs`]) for a game of one of `builds` (each a build id's start): the
/// builder's own asks for the supported build alone; a test's synthetic game is a build of its own
/// (builder/crates/synthetic-game). The game's name and icon are kept in `staging` (the finish's folder: the game
/// folder gets them at the swap).
pub fn read_exefs_of(
    game: &GameSource,
    exefs: &Path,
    staging: &Path,
    r: &Reporter,
    builds: &[&str],
) -> Result<GameRead> {
    // (made again from nothing: a module an earlier build or another dump left would be loaded with these)
    fsx::remove_tree(exefs)?;
    let mut read = GameRead::default();
    match game {
        GameSource::Nup(p) => {
            let n = Nup::open(p)?;
            let t = n.pick_title(None)?;
            // (beside its place, then renamed: a read stopped half way leaves no exefs that looks whole)
            let part = exefs.with_extension("part");
            fsx::remove_tree(&part)?;
            nup::extract(&n, t, &part, Some(Part::Exefs), &mut |_| {})?;
            std::fs::rename(part.join(Part::Exefs.name()), exefs).at(exefs)?;
            fsx::remove_tree(&part)?;
            // (the game's own name and icon: nothing here stops a build)
            match title::from_nup(&n) {
                Ok(Some((name, icon))) => {
                    title::keep(staging, &name, icon.as_deref())?;
                    read.title = Some(name);
                }
                Ok(None) => {}
                Err(e) => r.log(format!("the game's name and icon could not be read from the .nup ({e})")),
            }
        }
        GameSource::Folder { exefs: from, .. } => {
            checks::refuse_dump_links(from)?;
            fsx::copy_tree(from, exefs)?; // (unpacked in the copy: the player's dump stays as it is)
            for rel in fsx::files_under(exefs)? {
                nso::unpack_in_place(&exefs.join(rel))?;
            }
        }
    }
    checks::check_modules(exefs)?;
    checks::check_build_in(exefs, builds)?;
    read.build_id = checks::main_build_id(exefs)?;
    let (name, icon) = title::kept(staging);
    r.log(format!(
        "the game: build {}, named {}, {}",
        &read.build_id[..read.build_id.len().min(8)],
        name.as_deref().unwrap_or("Longshot"),
        if icon.is_some() { "with its icon" } else { "no icon" }
    ));
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;
    use build_core::report::Format;

    /// A stand-in dump's romfs: the names the check looks for, nothing of the game's.
    fn stand_in_dump(romfs: &Path) {
        std::fs::create_dir_all(romfs.join("Pack")).unwrap();
        std::fs::create_dir_all(romfs.join("System/Resource")).unwrap();
        std::fs::create_dir_all(romfs.join("System/HeapSize")).unwrap();
        std::fs::write(romfs.join("Pack/ZsDic.pack.zs"), b"stand-in").unwrap();
        std::fs::write(romfs.join("System/HeapSize/Default.Nin_Ounce_NVN2.HeapDef.byml.zs"), b"stand-in").unwrap();
    }

    #[test]
    fn a_dump_s_build_takes_no_romfs_folder_of_the_player_s() {
        let d = crate::scratch("romfs-theirs");
        let (dump, out, staging) = (d.join("dump/romfs"), d.join("out"), d.join("staging"));
        stand_in_dump(&dump);
        let game = GameSource::Folder { romfs: dump.clone(), exefs: d.join("dump/exefs") };
        let r = Reporter::to(Format::Json, Box::new(std::io::sink()));
        // (the player's own romfs folder, a mod's files in it: refused, and every file of it as it was)
        std::fs::create_dir_all(out.join("romfs/Pack")).unwrap();
        std::fs::write(out.join("romfs/Pack/ZsDic.pack.zs"), b"theirs").unwrap();
        let e = ReadStage.romfs(&game, &out, &staging, &r).unwrap_err();
        assert!(matches!(e, Error::Player { code: "romfs_folder_theirs", .. }), "{e}");
        assert_eq!(std::fs::read(out.join("romfs/Pack/ZsDic.pack.zs")).unwrap(), b"theirs");
        assert!(!crate::folder::romfs_made_here(&out), "not taken for the builder's");
        // (none there: made, and noted the builder's; a build after links it again)
        fsx::remove_tree(&out.join("romfs")).unwrap();
        ReadStage.romfs(&game, &out, &staging, &r).unwrap();
        assert!(crate::folder::romfs_made_here(&out));
        assert!(fsx::same_file(&out.join("romfs/Pack/ZsDic.pack.zs"), &dump.join("Pack/ZsDic.pack.zs")));
        ReadStage.romfs(&game, &out, &staging, &r).unwrap();
        // (an empty one, as a stopped build or the player's mkdir leaves it: nothing to lose, made the builder's)
        fsx::remove_tree(&out.join("romfs")).unwrap();
        std::fs::remove_file(out.join(crate::folder::RECORD_FILE)).unwrap(); // (its note: the record's files)
        std::fs::create_dir_all(out.join("romfs")).unwrap();
        ReadStage.romfs(&game, &out, &staging, &r).unwrap();
        assert!(crate::folder::romfs_made_here(&out));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A manifest mod at `path` under `profile`'s mods folder.
    fn manifest_mod(profile: &Path, path: &str) {
        let d = crate::play::mods(profile).join(path);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("mod.json"), b"{}").unwrap();
    }

    fn names(profile: &Path) -> Vec<(String, bool)> {
        manifest_mods(Some(profile)).into_iter().map(|(n, _, on)| (n, on)).collect()
    }

    #[test]
    fn manifest_mods_are_found_and_ordered_as_the_game_does() {
        assert!(manifest_mods(None).is_empty(), "no profile: none");
        let d = crate::scratch("manifest-mods");
        assert!(names(&d).is_empty(), "no mods folder: none");
        for m in ["b", "A", "c", "x/y", "1/2/3/4", "1/2/3/4b/5", "-off/z", "_off", ".hid", "s/-o", "a/inner"] {
            manifest_mod(&d, m);
        }
        let on = |n: &str| (n.to_string(), true);
        // (no mods.json: all on, by path ignoring case; four levels down found, five not; -, _ and . folders skipped)
        assert_eq!(names(&d), [on("1/2/3/4"), on("A"), on("a/inner"), on("b"), on("c"), on("x/y")]);
        // (nothing below a mod)
        manifest_mod(&d, "b/below");
        assert!(!names(&d).iter().any(|(n, _)| n == "b/below"));
        // (mods.json's order first, its switches kept, unknown or non-local ids ignored; the rest on, after)
        std::fs::write(
            d.join("mods.json"),
            r#"{"mods":[{"id":"local/x/y","enabled":true},{"id":"local/b","enabled":false},{"id":"local/gone"},{"id":"c"},{"id":"local/A"}]}"#,
        )
        .unwrap();
        assert_eq!(names(&d), [on("x/y"), ("b".into(), false), on("A"), on("1/2/3/4"), on("a/inner"), on("c")]);
        // (a mods.json that isn't such a list: every mod on, in path order)
        for bad in [&b"{not json"[..], br#"{"mods": 3}"#] {
            std::fs::write(d.join("mods.json"), bad).unwrap();
            assert_eq!(names(&d), [on("1/2/3/4"), on("A"), on("a/inner"), on("b"), on("c"), on("x/y")]);
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}

//! What the player's game files must be before a build starts, each said in the player's words with what to do: the
//! code modules, the supported build, the romfs's files, no links in a dump, the game folder's paths, a writable game
//! folder; and `check`'s answer (is this the version the builder supports?) without unpacking anything.

use crate::nso;
use crate::nup::{Nup, Part};
use crate::romfs::{self, Index};
use build_core::{Error, IoContext, Result};
use std::path::Path;

/// The build of the game the patches, the site tables and the profiles were made for (main's build id, its start).
pub const SUPPORTED_BUILD: &str = "e0f3ccfa";

/// Every module of an exefs is one the builder can read: an NSO (main.npdm aside, and a `.part` an unpack left).
pub fn check_modules(exefs: &Path) -> Result<()> {
    for entry in std::fs::read_dir(exefs).at(exefs)? {
        let entry = entry.at(exefs)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "main.npdm" || name.ends_with(".part") {
            continue;
        }
        let mut head = [0u8; 16];
        let n =
            std::fs::File::open(entry.path()).and_then(|f| crate::nup::read_at(&f, &mut head, 0)).at(entry.path())?;
        if head[..n.min(4)] != *b"NSO0" {
            return Err(Error::player(
                "not_a_module",
                format!(
                    "The file {name} in this dump is not a game module (it starts {}). Try another dump of the game, or send \
                     this message to the Longshot developers.",
                    build_core::key::to_hex(&head[..n])
                ),
            ));
        }
    }
    Ok(())
}

/// The words for another build than the supported one.
fn other_build(id: &str) -> String {
    format!("This is build {}. Longshot supports build {SUPPORTED_BUILD}.", &id[..id.len().min(8)])
}

/// main's build id, or the player told the dump has no game executable.
pub fn main_build_id(exefs: &Path) -> Result<String> {
    let main = exefs.join("main");
    nso::build_id(&main)?
        .ok_or_else(|| Error::player("not_a_module", format!("{} is not a game executable.", main.display())))
}

/// main is the build Longshot supports.
pub fn check_supported_build(exefs: &Path) -> Result<()> {
    check_build_in(exefs, &[SUPPORTED_BUILD])
}

/// main is one of `builds` (each a build id's start). A player's build asks for [`SUPPORTED_BUILD`] alone
/// ([`check_supported_build`]); a test builds its synthetic game, of a build of its own, through this.
pub fn check_build_in(exefs: &Path, builds: &[&str]) -> Result<()> {
    let id = main_build_id(exefs)?;
    if !builds.iter().any(|b| id.starts_with(b)) {
        return Err(Error::player("unsupported_build", other_build(&id)));
    }
    Ok(())
}

/// Why a kept exefs (a game folder's copy) cannot be built from, or `None`: the questions of [`check_modules`] and
/// [`check_supported_build`], answered without stopping the build.
pub fn exefs_problem(exefs: &Path) -> Option<String> {
    if !exefs.join("main").is_file() {
        return Some("no main".into());
    }
    if let Err(e) = check_modules(exefs) {
        return Some(e.to_string());
    }
    match main_build_id(exefs) {
        Ok(id) if id.starts_with(SUPPORTED_BUILD) => None,
        Ok(_) => Some("another build of the game".into()),
        Err(e) => Some(e.to_string()),
    }
}

/// `check`'s answer: (whether it is the supported build, main's build id short, the sentence). From a .nup, main's
/// header is read in place (nothing unpacked); else from an exefs folder.
pub fn check(nup: Option<&Path>, exefs: Option<&Path>) -> Result<(bool, String, String)> {
    let id = match (nup, exefs) {
        (Some(p), _) => {
            let nup = Nup::open(p)?;
            let t = nup.pick_title(None)?;
            let main =
                nup.part(t, Part::Exefs).into_iter().find(|(n, _)| n == "main").ok_or_else(|| {
                    Error::player("not_a_module", "This .nup has no game executable (no exefs/main).")
                })?;
            let head = nup.head(main.1, 0x60)?;
            if head.len() < 0x60 || head[..4] != *b"NSO0" {
                return Err(Error::player("not_a_module", "The .nup's exefs/main is not a game executable."));
            }
            build_core::key::to_hex(&head[0x40..0x60])
        }
        (None, Some(e)) => main_build_id(e)?,
        (None, None) => return Err(Error::player("no_game_files", "Choose your .nup, or the dump's exefs folder.")),
    };
    let short = id[..8.min(id.len())].to_string();
    Ok(if id.starts_with(SUPPORTED_BUILD) {
        (true, short.clone(), format!("This is the version Longshot supports (build {short})."))
    } else {
        (false, short, other_build(&id))
    })
}

// ---- the romfs

/// Where the build reads the romfs: a folder (a developer's unpacked dump) or the index of the player's .nup.
pub enum Romfs<'a> {
    /// An unpacked romfs folder.
    Folder(&'a Path),
    /// The .nup's romfs, by its index.
    Index(&'a Index),
}

/// What a romfs must hold for the build and the game's start, each as (any one of its names, a folder?): the build
/// reads Pack/ZsDic.pack.zs and System/Resource, and the game cannot start without its heap definition (a romfs with
/// no System folder crashes at boot, its heaps set up without their sizes).
const NEEDED: [(&[&str], bool); 3] = [
    (&["Pack/ZsDic.pack.zs"], false),
    (&["System/Resource"], true),
    (
        &[
            "System/HeapSize/Product.Nin_Ounce_NVN2.HeapDef.byml.zs",
            "System/HeapSize/Default.Nin_Ounce_NVN2.HeapDef.byml.zs",
        ],
        false,
    ),
];

/// The first of what a romfs must hold that it lacks (its first name), or `None`.
pub fn romfs_missing(romfs: &Romfs) -> Option<&'static str> {
    let there = |name: &str, dir: bool| match romfs {
        Romfs::Folder(f) => {
            let p = f.join(name);
            if dir { p.is_dir() } else { p.is_file() }
        }
        Romfs::Index(i) => {
            if dir {
                i.is_dir(name)
            } else {
                i.find(name).is_some()
            }
        }
    };
    NEEDED.iter().find(|(names, dir)| !names.iter().any(|n| there(n, *dir))).map(|(names, _)| names[0])
}

/// A chosen romfs folder holds what the build and the game's start need, said before the game folder is made
/// (another tool's packed copy, a romfs.bin, would build for minutes into many GB, and the game stop at boot).
pub fn check_romfs_folder(romfs: &Path) -> Result<()> {
    if let Some(missing) = romfs_missing(&Romfs::Folder(romfs)) {
        let packed = if romfs.join("romfs.bin").is_file() {
            " It holds a romfs.bin, a packed copy of the game's files: extract it first."
        } else {
            ""
        };
        return Err(Error::player(
            "romfs_incomplete",
            format!(
                "The romfs folder {} does not hold the game's files (no {missing}): choose the extracted RomFS.{packed}",
                romfs.display()
            ),
        ));
    }
    Ok(())
}

/// The same for the romfs of a .nup or a game folder's copy (`source` says which): a game built from it could not
/// start.
pub fn check_game_romfs(romfs: &Romfs, source: &str) -> Result<()> {
    if let Some(missing) = romfs_missing(romfs) {
        return Err(Error::player(
            "romfs_incomplete",
            format!(
                "{source} does not hold all of the game's files (no {missing}): a game built from it could not start. Dump \
                 the game again and choose the complete dump."
            ),
        ));
    }
    Ok(())
}

/// An extracted dump holds only files and folders: a link in it would bring the file it names (anywhere on this PC)
/// into the game folder. The folder itself may be one. Walked without recursion.
pub fn refuse_dump_links(folder: &Path) -> Result<()> {
    let mut pending = vec![folder.to_path_buf()];
    while let Some(d) = pending.pop() {
        let listing = std::fs::read_dir(&d).map_err(|e| {
            Error::player(
                "dump_unreadable",
                format!("Could not read the game files at {} ({}): {e}", folder.display(), d.display()),
            )
        })?;
        for entry in listing {
            let entry = entry.at(&d)?;
            let meta = std::fs::symlink_metadata(entry.path()).at(entry.path())?;
            if is_link(&meta) {
                return Err(Error::player(
                    "dump_has_links",
                    format!(
                        "The game files at {} contain a link ({}): copy them without links and choose that folder.",
                        folder.display(),
                        entry.path().display()
                    ),
                ));
            }
            if meta.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

/// A link: a symbolic link, or on Windows any reparse point (a junction too).
pub(crate) fn is_link(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// The game folder takes new files (a folder under Program Files does only for administrators).
pub fn check_writable(game: &Path) -> Result<()> {
    let probe = game.join(".longshot_write_check");
    // (a file of its own: create_new follows no link left at that name, and refuses whatever is there)
    let _ = std::fs::remove_file(&probe);
    let tried = std::fs::create_dir_all(game)
        .and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&probe))
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"ok"))
        .and_then(|()| std::fs::remove_file(&probe));
    let Err(e) = tried else { return Ok(()) };
    let protected = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "SystemRoot"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|d| std::path::absolute(game).is_ok_and(|g| g.starts_with(&d)));
    let where_ = if protected && e.kind() == std::io::ErrorKind::PermissionDenied {
        " Windows protects folders under Program Files and Windows, and only lets administrators write there."
    } else {
        ""
    };
    let example = if cfg!(windows) { r"C:\Games\Longshot" } else { "~/Games/Longshot" };
    Err(Error::player(
        "game_folder_not_writable",
        format!(
            "The game folder {} can't be written to ({e}).{where_} Choose a game folder of your own, for example {example}, \
             and build again.",
            game.display()
        ),
    ))
}

// ---- a damaged dump (the .nup's damaged files and their stand-ins: crate::romfs)

/// The damaged-files note in the game folder: what the index reads in place of each damaged file.
pub const DAMAGED_NOTE: &str = "longshot_damaged_files.txt";

/// The damaged-files note's text: what an incomplete dump is, then each file and what is read in its place.
pub fn damaged_note(done: &[romfs::StandIn]) -> String {
    let n = done.len();
    let too_many = n > romfs::MOST_STAND_INS;
    let mut lines = vec![format!(
        "Your game dump is incomplete: {n} game file{} in it {} damaged (a complete dump is about 23.8 GB, an incomplete one \
         23.3 GB). {} Dump the game again for the original files:",
        if n == 1 { "" } else { "s" },
        if n == 1 { "is" } else { "are" },
        if too_many {
            "That is too many to replace with similar files (an incomplete dump has a few)."
        } else {
            "Similar files are used where available, with different textures in those places."
        }
    )];
    for s in done {
        lines.push(match &s.with {
            Some(w) => format!("  {}: in its place {}", s.damaged, w.rsplit('/').next().unwrap_or(w)),
            None => format!("  {}: no replacement found; build cannot continue", s.damaged),
        });
    }
    lines.join("\n") + "\n"
}

/// The build stops when a damaged file has no stand-in.
pub fn unresolved(done: &[romfs::StandIn]) -> Result<()> {
    let n = done.iter().filter(|s| s.with.is_none()).count();
    if n == 0 {
        return Ok(());
    }
    Err(Error::player(
        "dump_damaged",
        format!(
            "The game dump still has {n} damaged file{} with no replacement. The build stopped before compiling. Dump the \
             game again and choose the complete dump. The affected files are listed in {DAMAGED_NOTE} in the game folder.",
            if n == 1 { "" } else { "s" }
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nup::tests::{sample, scratch};

    #[test]
    fn modules_and_builds() {
        let d = scratch("checks-modules");
        std::fs::write(d.join("main"), crate::nso::tests::synthetic(0xe0, false)).unwrap();
        std::fs::write(d.join("main.npdm"), b"META").unwrap();
        check_modules(&d).unwrap();
        // (the synthetic main's id is e0 and zeros: not the supported build)
        let e = check_supported_build(&d).unwrap_err();
        assert_eq!(e.code(), "unsupported_build");
        assert_eq!(e.to_string(), "This is build e0000000. Longshot supports build e0f3ccfa.");
        assert_eq!(exefs_problem(&d).as_deref(), Some("another build of the game"));
        check_build_in(&d, &["e000"]).unwrap();
        assert_eq!(check_build_in(&d, &["e0f3", "e1"]).unwrap_err().code(), "unsupported_build");
        std::fs::write(d.join("sdk"), b"garbage!").unwrap();
        assert_eq!(check_modules(&d).unwrap_err().code(), "not_a_module");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn check_reads_main_from_the_package() {
        let d = scratch("checks-check");
        let p = d.join("g.nup");
        std::fs::write(&p, sample().build()).unwrap();
        let (ok, build, text) = check(Some(&p), None).unwrap();
        assert!(!ok);
        assert_eq!(build, "e0000000");
        assert!(text.starts_with("This is build e0000000."));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_romfs_needs_its_system_files() {
        let d = scratch("checks-romfs");
        assert_eq!(romfs_missing(&Romfs::Folder(&d)), Some("Pack/ZsDic.pack.zs"));
        std::fs::write(d.join("romfs.bin"), b"").unwrap();
        assert!(check_romfs_folder(&d).unwrap_err().to_string().contains("extract it first"));
        for f in ["Pack/ZsDic.pack.zs", "System/HeapSize/Default.Nin_Ounce_NVN2.HeapDef.byml.zs"] {
            std::fs::create_dir_all(d.join(f).parent().unwrap()).unwrap();
            std::fs::write(d.join(f), b"x").unwrap();
        }
        assert_eq!(romfs_missing(&Romfs::Folder(&d)), Some("System/Resource"));
        std::fs::create_dir_all(d.join("System/Resource")).unwrap();
        check_romfs_folder(&d).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn links_are_refused() {
        let d = scratch("checks-links");
        std::fs::create_dir_all(d.join("dump/a")).unwrap();
        refuse_dump_links(&d.join("dump")).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", d.join("dump/a/x")).unwrap();
        assert_eq!(refuse_dump_links(&d.join("dump")).unwrap_err().code(), "dump_has_links");
        std::fs::remove_dir_all(&d).unwrap();
    }
}

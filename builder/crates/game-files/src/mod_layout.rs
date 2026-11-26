//! Which mods in a profile's `mods/` are in the emulators' layout, and not the one mod format the builder and the game
//! read: a manifest mod, a folder with `mod.json` (docs/dev/reference/mods.md).
//!
//! The emulators' layout is a folder holding `romfs/`, `exefs/` or `cheats/` (bare in `mods/` itself, a folder of it, or
//! deeper, as emulators keep them: `local/<name>/`, `contents/<title id>/<name>/`), with no `mod.json`. Such a mod is
//! converted once, at import, into a manifest mod (`Longshot.exe mod import <folder>`, the Mod API's modtool); until
//! then nothing of it is built in or loaded, and the builder says so ([`needing_import`]: the `mod_needs_import`
//! warning).

use std::path::Path;

/// A manifest mod's file, beside its own `romfs/`, `exefs/` and the rest.
pub const MANIFEST: &str = "mod.json";
/// The folders that make a folder a mod in the emulators' layout.
pub const LAYOUT: [&str; 3] = ["romfs", "exefs", "cheats"];
/// How deep below the mods folder a mod in the emulators' layout is looked for (the old loader's limit).
pub const DEPTH: usize = 4;
/// The command that converts one, for the player.
pub const IMPORT: &str = "Longshot.exe mod import";

/// The mods in the emulators' layout under `mods` (a profile's `mods/`), as their folders from it (`/` between; `.`
/// for `mods` itself, bare `romfs/` or `exefs/` there), by name. A manifest mod (a folder with [`MANIFEST`]) is not one,
/// and is not looked into; nor is a folder whose name starts with `.`. None when `mods` is not a folder.
pub fn needing_import(mods: &Path) -> Vec<String> {
    fn is_mod(d: &Path) -> bool {
        LAYOUT.iter().any(|p| d.join(p).is_dir())
    }
    fn walk(d: &Path, name: &str, depth: usize, out: &mut Vec<String>) {
        if d.join(MANIFEST).is_file() {
            return;
        }
        if is_mod(d) {
            out.push(if name.is_empty() { ".".to_string() } else { name.to_string() });
            return;
        }
        if depth >= DEPTH {
            return;
        }
        let Ok(listing) = std::fs::read_dir(d) else { return };
        let mut entries: Vec<String> =
            listing.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        entries.sort();
        for e in entries {
            if e.starts_with('.') || !d.join(&e).is_dir() {
                continue;
            }
            let n = if name.is_empty() { e.clone() } else { format!("{name}/{e}") };
            walk(&d.join(&e), &n, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    if mods.is_dir() {
        walk(mods, "", 0, &mut out);
    }
    out
}

/// The warning's sentence for the player: the mods of [`needing_import`] under `mods`, and how to import them.
pub fn import_text(mods: &Path, found: &[String]) -> String {
    let shown: Vec<String> = found.iter().take(5).map(|f| mods.join(f).display().to_string()).collect();
    let more =
        if found.len() > shown.len() { format!(" and {} more", found.len() - shown.len()) } else { String::new() };
    format!(
        "{} mod(s) in {} are in an emulator's layout (romfs, exefs or cheats folders with no {MANIFEST}), which Longshot \
         no longer loads or builds in: {}{more}. Import each once with `{IMPORT} <folder>`, which makes it a Longshot \
         mod; until then it is left out.",
        found.len(),
        mods.display(),
        shown.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mods_in_the_emulators_layout_are_found_and_manifest_mods_are_not() {
        let d = crate::nup::tests::scratch("mod-layout");
        let mods = d.join("mods");
        assert!(needing_import(&mods).is_empty(), "no mods folder: none");
        for m in [
            "Zed/exefs",
            "alpha/romfs",
            "local/pack/romfs",
            "contents/0100/name/cheats",
            "_off/exefs",
            "manifest/romfs",
            "manifest/inner/exefs",
            ".hidden/romfs",
            "empty",
            "a/b/c/d/e/romfs",
        ] {
            std::fs::create_dir_all(mods.join(m)).unwrap();
        }
        std::fs::write(mods.join("manifest").join(MANIFEST), b"{}").unwrap();
        std::fs::write(mods.join("alpha/exefs"), b"a file, not a folder").unwrap();
        let found = needing_import(&mods);
        assert_eq!(found, ["Zed", "_off", "alpha", "contents/0100/name", "local/pack"]);
        let text = import_text(&mods, &found);
        assert!(text.starts_with("5 mod(s) in ") && text.contains(IMPORT) && text.contains("local"), "{text}");
        // (bare romfs/ in the mods folder itself: the folder is one such mod)
        std::fs::create_dir_all(mods.join("romfs")).unwrap();
        assert_eq!(needing_import(&mods), ["."]);
        let many: Vec<String> = (0..7).map(|i| format!("m{i}")).collect();
        assert!(import_text(&mods, &many).contains(" and 2 more."));
        std::fs::remove_dir_all(&d).unwrap();
    }
}

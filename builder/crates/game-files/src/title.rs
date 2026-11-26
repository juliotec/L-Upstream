//! The game's own name and icon, from its .nup, for the window's title and the name Windows shows for the exe.
//!
//! A title's metadata (at +0x20 of its record, [`crate::nup`]) is a list of records, each u32 kind, u32 size, then its
//! bytes: 1 the NPDM, 3 the icon (a JPEG), 4 the version ("1.1.1"), 5 the name, 8 a short name. What is read is kept in
//! the game folder, so an update built from the folder (with no .nup) has them too, and the game reads the title from
//! there: `longshot_title.txt` (its first line: host/hos_paths.h hos_game_title) and `.longshot_icon.jpg`. Without a
//! name the game is "Longshot".

use crate::nup::{Nup, read_at};
use build_core::{IoContext, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// The window title's file in the game folder.
pub const TITLE_FILE: &str = "longshot_title.txt";
/// The icon's file in the game folder.
pub const ICON_FILE: &str = ".longshot_icon.jpg";
/// What follows the game's name in the window's title.
pub const SUFFIX: &str = " (Longshot)";
const ICON: u32 = 3;
const NAME: u32 = 5;

/// The names as the games write them, by title id: a .nup's names are fit for a file name (its dumper drops what
/// Windows refuses in one, so the colon of the name the game shows is gone).
fn known_name(id: u64) -> Option<&'static str> {
    (id == 0x0400_4860_24DE_C000).then_some("The Legend of Zelda: Ocarina of Time")
}

/// A title's metadata records, by kind (the first of each); what does not parse ends the list.
pub fn records(blob: &[u8]) -> BTreeMap<u32, &[u8]> {
    let mut out = BTreeMap::new();
    let mut at = 0usize;
    while at + 8 <= blob.len() {
        let kind = u32::from_le_bytes(blob[at..at + 4].try_into().unwrap_or_default());
        let size = u32::from_le_bytes(blob[at + 4..at + 8].try_into().unwrap_or_default()) as usize;
        let Some(body) = blob.get(at + 8..at + 8 + size) else { break };
        out.entry(kind).or_insert(body);
        at += 8 + size;
    }
    out
}

/// One line of printable text, at most 200 characters.
pub fn clean(name: &str) -> String {
    let kept: String = name.chars().filter(|c| !c.is_control()).collect();
    kept.trim().chars().take(200).collect()
}

/// The game's name and icon (a JPEG), from the newest title of the .nup; `None` when it has neither.
pub fn from_nup(nup: &Nup) -> Result<Option<(String, Option<Vec<u8>>)>> {
    let t = nup.pick_title(None)?;
    let (offset, size) = t.meta;
    if size == 0 || size > 16 << 20 || offset.saturating_add(size) > nup.len {
        return Ok(None);
    }
    let mut blob = vec![0u8; size as usize];
    let n = read_at(nup.file(), &mut blob, offset).at(nup.path())?;
    blob.truncate(n);
    let meta = records(&blob);
    let name = known_name(t.id)
        .map(str::to_string)
        .unwrap_or_else(|| clean(&String::from_utf8_lossy(meta.get(&NAME).copied().unwrap_or_default())));
    let icon = meta.get(&ICON).filter(|i| i.starts_with(&[0xff, 0xd8, 0xff])).map(|i| i.to_vec());
    if name.is_empty() && icon.is_none() {
        return Ok(None);
    }
    Ok(Some((name, icon)))
}

/// The game folder's copy of what was read (each file written only when it changed).
pub fn keep(game: &Path, name: &str, icon: Option<&[u8]>) -> Result<()> {
    if !name.is_empty() {
        build_core::fsx::write_if_changed(&game.join(TITLE_FILE), format!("{name}{SUFFIX}\n").as_bytes())?;
    }
    if let Some(icon) = icon {
        build_core::fsx::write_if_changed(&game.join(ICON_FILE), icon)?;
    }
    Ok(())
}

/// The window title and the icon the folder has.
pub fn kept(game: &Path) -> (Option<String>, Option<Vec<u8>>) {
    let title = std::fs::read_to_string(game.join(TITLE_FILE))
        .ok()
        .and_then(|t| Some(clean(t.lines().next()?)))
        .filter(|t| !t.is_empty());
    let icon = std::fs::read(game.join(ICON_FILE)).ok().filter(|i| !i.is_empty());
    (title, icon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_names() {
        let mut blob = Vec::new();
        for (k, b) in [(4u32, &b"1.1.1"[..]), (5, b"Game\x07 Name "), (5, b"second")] {
            blob.extend_from_slice(&k.to_le_bytes());
            blob.extend_from_slice(&(b.len() as u32).to_le_bytes());
            blob.extend_from_slice(b);
        }
        blob.extend_from_slice(&[9, 0, 0, 0, 0xff, 0, 0, 0]); // (cut short: ends the list)
        let r = records(&blob);
        assert_eq!(r.len(), 2);
        assert_eq!(clean(&String::from_utf8_lossy(r[&5])), "Game Name");
        assert_eq!(known_name(0x0400486024dec000), Some("The Legend of Zelda: Ocarina of Time"));
    }

    #[test]
    fn kept_in_the_folder() {
        let d = std::env::temp_dir().join(format!("game-files-title-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(kept(&d), (None, None));
        keep(&d, "A Game", Some(&[0xff, 0xd8, 0xff, 1])).unwrap();
        assert_eq!(kept(&d), (Some("A Game (Longshot)".into()), Some(vec![0xff, 0xd8, 0xff, 1])));
        std::fs::remove_dir_all(&d).unwrap();
    }
}

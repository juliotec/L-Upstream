//! The game's texts with the console's wording made the PC's.
//!
//! One of the game's texts names the console: the hint for playing the ocarina by microphone says to use its built-in
//! one. On a PC that is the wrong advice (the game listens to the microphone the menu's Microphone row chooses), so the
//! string is reworded in the player's own text archives at the build, and `game/texts.c` serves the result in place of
//! the game's file (a mod's own file wins; `HOS_TEXTS=0`: the game's own texts).
//!
//! The texts are `romfs/Mals/<language>.Product.<n>.sarc.zs`, one archive a language: a zstd frame (made with the
//! game's dictionary) of a SARC of MSBT files, UTF-8 in this game. [`STRINGS`] names each string by its MSBT and its
//! label, and gives its edit for every wording of it the game has: the bytes from start to end of the string become the
//! replacement, the rest (the other sentences, the menu's icon, the reading aids over the kanji) stays the game's. A
//! wording is known by its fingerprint, so no game text is in this file, and a string that is not one of them (another
//! game version, a text a mod's author changed) is left as it is and said.
//!
//! A replacement is never longer than what it replaces: the string is rewritten in its place and what is left of its
//! room filled with NULs, so no offset in the MSBT or the SARC moves and the archive keeps its size, which the game's
//! resource size table goes by.
//!
//! Every language whose string was reworded gets `<out>/<the archive's name in lower case>` (`game/texts.c` looks it
//! up by the romfs path's lower-case key), a zstd frame made without the dictionary, its size in its header; the
//! others keep the game's own.

use crate::cache::Cache;
use crate::romfs::Romfs;
use crate::sarc;
use crate::zs::{self, Dictionaries};
use build_core::key::{Key, sha1_hex};
use build_core::report::Reporter;
use build_core::{Error, IoContext, Result, fsx};
use std::path::Path;

/// The recipe's version: raised when the same archives would be reworded otherwise ([`crate::cache`]).
const RECIPE: &str = "texts 1";

/// One string to reword: its MSBT, its label, and for each wording of it the game has (by [`fingerprint`]) the bytes
/// `start..end` of its room and what they become.
pub struct Reword<'a> {
    /// The MSBT, in the archive.
    pub msbt: &'a str,
    /// The string's label.
    pub label: &'a str,
    /// (fingerprint, start, end, replacement) for each wording.
    pub edits: &'a [(&'a str, usize, usize, &'a str)],
}

/// The strings reworded.
pub const STRINGS: &[Reword<'static>] = &[
    // the "Ocarina Audio Input" hint: the console's built-in microphone -> your microphone (the languages with that wording)
    Reword {
        msbt: "LayoutMsg/ActionGuide_00.msbt",
        label: "Pa_ActionGuide_01-T_Description_00",
        edits: &[
            ("b816b30394a86b8a", 4, 38, "your microphone"),     // USen, EUen
            ("9925b429328b1fd2", 21, 69, "votre microphone\n"), // USfr, EUfr
            ("e4bd22fcea2f8529", 16, 60, "un micrófono"),       // USes, EUes
            ("0dd1fb25d15aec84", 4, 46, "o seu microfone"),     // USpt
            ("3179003a18de9124", 9, 64, "dein Mikrofon"),       // EUde
            ("7fa2434137fc436c", 11, 54, "il tuo microfono"),   // EUit
            ("7f5339bb84e3bb77", 8, 56, "je microfoon"),        // EUnl
            ("21bce07f220b2409", 8, 57, "mikrofonowi"),         // EUpl
            ("54935b14b603297c", 0, 55, "Микрофон"),            // EUru
            ("ed6bdf6d1550bdad", 0, 50, ""),                    // JPja: "the console's" goes, the microphone stays
            ("01467ba53d125ec9", 0, 28, ""),                    // KRko: the same
            ("cd07456de7b137ad", 6, 32, ""),                    // CNzh: the same
            ("c52f433f130e10eb", 6, 32, ""),                    // TWzh: the same
        ],
    },
];

/// A wording's name in [`STRINGS`]: the first 16 hex digits of the SHA-1 of the string's whole room, its terminator
/// too.
pub fn fingerprint(room: &[u8]) -> String {
    sha1_hex(room)[..16].to_string()
}

fn u32_at(m: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(m.get(at..at.checked_add(4)?)?.try_into().ok()?) as usize)
}

/// An MSBT's strings by label: each string's room (its bytes, tags and terminator, up to the next string's start).
/// `None` when it is not a little-endian UTF-8 MSBT with labels and texts.
pub fn msbt_strings(m: &[u8]) -> Option<Vec<(String, std::ops::Range<usize>)>> {
    if m.get(..8)? != b"MsgStdBn" || m.get(8..10)? != [0xFF, 0xFE] || *m.get(12)? != 0 {
        return None;
    }
    let (mut at, mut lbl, mut txt) = (0x20usize, None, None);
    while at + 16 <= m.len() {
        let size = u32_at(m, at + 4)?;
        match &m[at..at + 4] {
            b"LBL1" => lbl = Some(at + 16),
            b"TXT2" => txt = Some((at + 16, size)),
            _ => {}
        }
        at = (at + 16 + size + 15) & !15;
    }
    let (lbl, (txt, txt_size)) = (lbl?, txt?);
    // (each string's start is 4 bytes of the table: no more of them than the data holds)
    let count = u32_at(m, txt)?;
    if count > m.len().saturating_sub(txt + 4) / 4 {
        return None;
    }
    let mut starts = Vec::with_capacity(count + 1);
    for i in 0..count {
        starts.push(u32_at(m, txt + 4 + 4 * i)?);
    }
    starts.push(txt_size);
    let mut out = Vec::new();
    // (a label is 5 bytes at least: slots that name the same labels again and again stop at as many as the data holds)
    let mut most = m.len() / 5;
    for slot in 0..u32_at(m, lbl)?.min(m.len() / 8) {
        let n = u32_at(m, lbl + 4 + 8 * slot)?;
        let mut at = lbl.checked_add(u32_at(m, lbl + 8 + 8 * slot)?)?;
        for _ in 0..n {
            most = most.checked_sub(1)?;
            let len = usize::from(*m.get(at)?);
            let index = u32_at(m, at + 1 + len)?;
            if index < count && starts[index] <= starts[index + 1] {
                let label = String::from_utf8_lossy(m.get(at + 1..at + 1 + len)?).into_owned();
                out.push((label, txt + starts[index]..txt + starts[index + 1]));
            }
            at += 5 + len;
        }
    }
    Some(out)
}

/// What [`reword`] did to an archive.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reworded {
    /// The labels reworded.
    pub done: Vec<String>,
    /// The labels left as they are, each with why.
    pub left: Vec<(String, String)>,
}

/// A text archive (unpacked) with `strings`' edits made in place: its length stays.
pub fn reword(data: &mut [u8], strings: &[Reword<'_>], what: &str) -> Result<Reworded> {
    let mut out = Reworded::default();
    let files = sarc::entries(data, what)?;
    for s in strings {
        let Some(file) = files.iter().find(|e| e.name.as_deref() == Some(s.msbt)) else {
            out.left.push((s.label.into(), format!("no {}", s.msbt)));
            continue;
        };
        let base = file.range.start;
        let room = msbt_strings(&data[file.range.clone()])
            .and_then(|rooms| rooms.into_iter().find(|(l, _)| l == s.label))
            .map(|(_, r)| base + r.start..base + r.end);
        let Some(room) = room.filter(|r| r.start <= r.end && r.end <= file.range.end) else {
            out.left.push((s.label.into(), "no such string".into()));
            continue;
        };
        let text = data[room.clone()].to_vec();
        let print = fingerprint(&text);
        let Some(&(_, start, stop, new)) = s.edits.iter().find(|e| e.0 == print) else {
            out.left.push((s.label.into(), "its text is not one this was written for".into()));
            continue;
        };
        if start > stop || stop >= text.len() || new.len() > stop - start {
            return Err(Error::format(what, format!("{}: the replacement does not fit what it replaces", s.label)));
        }
        let mut made = text[..start].to_vec();
        made.extend_from_slice(new.as_bytes());
        made.extend_from_slice(&text[stop..]);
        made.resize(text.len(), 0); // (in its own room: nothing after it moves)
        data[room].copy_from_slice(&made);
        out.done.push(s.label.into());
    }
    Ok(out)
}

/// The game's text archives: `Mals/*.sarc.zs`, by name.
pub fn archives(romfs: &dyn Romfs) -> Result<Vec<String>> {
    Ok(romfs
        .files_under("Mals")?
        .into_iter()
        .filter(|f| f.ends_with(".sarc.zs") && !f["Mals/".len()..].contains('/'))
        .collect())
}

/// `out/<archive, lower case>` for each language with a string reworded, each kept when its archive and this recipe
/// are as they were ([`crate::cache`]). Files of an earlier build this one does not make are removed (and `out` when
/// none is made). The names made.
pub fn make(romfs: &dyn Romfs, out: &Path, cache: &Cache, report: &Reporter) -> Result<Vec<String>> {
    let mut made = Vec::new();
    let mut dictionaries = None;
    for rel in archives(romfs)? {
        // (a name that is not a safe path on every system, a device's on Windows (CON), is no archive of the game's:
        // its file in out would be another place. A .nup's names are checked so when it is read; a folder's are not)
        if game_files::nup::package_parts(&rel).is_err() {
            report.log(format!("texts: {rel:?} left out: not a safe file name"));
            continue;
        }
        let name = rel.rsplit('/').next().unwrap_or_default().to_string();
        let lower = name.to_lowercase();
        let target = out.join(&lower);
        let mut key = Key::new();
        key.text("recipe", RECIPE);
        cache.key_romfs(&mut key, romfs, &rel)?;
        cache.key_romfs(&mut key, romfs, zs::DICTIONARY_PACK)?;
        let stamp = format!("texts.{lower}");
        if cache.fresh(&stamp, &key) {
            made.push(lower);
            continue;
        }
        cache.forget(&stamp)?;
        if dictionaries.is_none() {
            dictionaries = Some(Dictionaries::load(romfs)?);
        }
        let mut data = dictionaries.as_ref().map_or_else(|| unreachable!(), |d| d.read(romfs, &rel))?;
        let result = reword(&mut data, STRINGS, &name)?;
        for (label, why) in &result.left {
            report.log(format!("texts: {name}: {label} left as it is ({why})"));
        }
        if result.done.is_empty() {
            continue;
        }
        fsx::write_atomic(&target, &zs::compress(&data))?;
        cache.record(&stamp, &key, std::slice::from_ref(&target))?;
        made.push(lower);
    }
    if out.is_dir() {
        for e in std::fs::read_dir(out).at(out)? {
            let e = e.at(out)?;
            let n = e.file_name().to_string_lossy().into_owned();
            if !made.contains(&n) {
                fsx::remove_file(&e.path())?;
            }
        }
        if made.is_empty() {
            let _ = std::fs::remove_dir(out);
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An MSBT of `slots` label slots that each name all of `labels` empty labels, and a TXT2 of `count` strings
    /// (its table as long as it says only when `whole`).
    fn crafted(slots: u32, labels: u32, count: u32, whole: bool) -> Vec<u8> {
        let mut m = b"MsgStdBn".to_vec();
        m.extend_from_slice(&[0xFF, 0xFE, 0, 0, 0, 3, 2, 0]);
        m.resize(0x20, 0);
        let mut lbl = slots.to_le_bytes().to_vec();
        let first = 4 + 8 * slots;
        for _ in 0..slots {
            lbl.extend_from_slice(&labels.to_le_bytes());
            lbl.extend_from_slice(&first.to_le_bytes());
        }
        for _ in 0..labels {
            lbl.extend_from_slice(&[0, 0, 0, 0, 0]); // (an empty label, string 0)
        }
        for (magic, body) in [
            (&b"LBL1"[..], lbl),
            (&b"TXT2"[..], {
                let mut t = count.to_le_bytes().to_vec();
                t.resize(if whole { 4 + 4 * count as usize } else { 8 }, 0);
                t
            }),
        ] {
            m.extend_from_slice(magic);
            m.extend_from_slice(&(body.len() as u32).to_le_bytes());
            m.resize(m.len() + 8, 0);
            m.extend_from_slice(&body);
            m.resize(m.len().div_ceil(16) * 16, 0);
        }
        m
    }

    #[test]
    fn a_crafted_string_table_is_refused_not_allocated() {
        // (four billion strings stated in 8 bytes: before, a table of them was reserved at once)
        assert_eq!(msbt_strings(&crafted(1, 1, u32::MAX, false)), None);
    }

    #[test]
    fn labels_named_again_and_again_stop_at_what_the_data_holds() {
        // (10,000 slots each naming the same 10,000 labels: 100 million before, each a string)
        let started = std::time::Instant::now();
        assert_eq!(msbt_strings(&crafted(10_000, 10_000, 1, true)), None);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(msbt_strings(&crafted(2, 3, 1, true)).map(|v| v.len()), Some(6), "a table that fits is read");
    }

    /// An MSBT with one label `label` on a string of `text`.
    fn msbt(label: &str, text: &[u8]) -> Vec<u8> {
        let mut m = b"MsgStdBn".to_vec();
        m.extend_from_slice(&[0xFF, 0xFE, 0, 0, 0, 3, 2, 0]); // (BOM, 0, UTF-8, version, sections)
        m.resize(0x20, 0);
        let section = |m: &mut Vec<u8>, magic: &[u8], body: &[u8]| {
            m.extend_from_slice(magic);
            m.extend_from_slice(&(body.len() as u32).to_le_bytes());
            m.resize(m.len() + 8, 0);
            m.extend_from_slice(body);
            while !m.len().is_multiple_of(16) {
                m.push(0xAB);
            }
        };
        let mut lbl = 1u32.to_le_bytes().to_vec(); // (one slot)
        lbl.extend_from_slice(&1u32.to_le_bytes());
        lbl.extend_from_slice(&12u32.to_le_bytes());
        lbl.push(label.len() as u8);
        lbl.extend_from_slice(label.as_bytes());
        lbl.extend_from_slice(&0u32.to_le_bytes());
        section(&mut m, b"LBL1", &lbl);
        let mut txt = 1u32.to_le_bytes().to_vec();
        txt.extend_from_slice(&8u32.to_le_bytes());
        txt.extend_from_slice(text);
        section(&mut m, b"TXT2", &txt);
        m
    }

    #[test]
    fn a_known_wording_is_reworded_in_its_room() {
        let text = b"Use the built-in mic.\0";
        let room = fingerprint(text);
        let edits = [(room.as_str(), 8, 20, "PC mic")];
        let strings = [Reword { msbt: "A.msbt", label: "L", edits: &edits }];
        let m = msbt("L", text);
        let mut archive = crate::sarc::tests::build(&[("A.msbt", &m)]);
        let before = archive.len();
        let got = reword(&mut archive, &strings, "test").unwrap();
        assert_eq!(got.done, ["L"]);
        assert_eq!(archive.len(), before);
        let file = &crate::sarc::entries(&archive, "t").unwrap()[0];
        let rooms = msbt_strings(&archive[file.range.clone()]).unwrap();
        let r = &rooms[0].1;
        let new = &archive[file.range.start + r.start..file.range.start + r.end];
        assert_eq!(new, b"Use the PC mic.\0\0\0\0\0\0\0");
        // (a second time the wording is not the one known: left)
        let again = reword(&mut archive, &strings, "test").unwrap();
        assert_eq!(again.left, [("L".to_string(), "its text is not one this was written for".to_string())]);
    }

    #[test]
    fn make_writes_the_languages_reworded_and_keeps_them_while_their_archive_stays() {
        let s = &STRINGS[0];
        let m = msbt(s.label, b"no wording known\0");
        let archive = crate::sarc::tests::build(&[(s.msbt, &m)]);
        let pack = crate::zs::compress(&crate::sarc::tests::build(&[]));
        let romfs = crate::romfs::tests::Memory::default()
            .with(crate::zs::DICTIONARY_PACK, pack)
            .with("Mals/USen.Product.110.sarc.zs", crate::zs::compress(&archive));
        let d = std::env::temp_dir().join(format!("build-assets-texts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let cache = Cache::open(&d.join("cache"));
        let report = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
        std::fs::create_dir_all(d.join("texts")).unwrap();
        std::fs::write(d.join("texts/old.sarc.zs"), b"x").unwrap();
        // (no wording this knows: nothing made, and an earlier build's file goes)
        assert!(make(&romfs, &d.join("texts"), &cache, &report).unwrap().is_empty());
        assert!(!d.join("texts").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_archive_of_an_unsafe_name_is_left_out() {
        let pack = crate::zs::compress(&crate::sarc::tests::build(&[]));
        let romfs = crate::romfs::tests::Memory::default()
            .with(crate::zs::DICTIONARY_PACK, pack)
            .with("Mals/CON.sarc.zs", b"not even an archive".to_vec());
        let d = std::env::temp_dir().join(format!("build-assets-texts-unsafe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let cache = Cache::open(&d.join("cache"));
        let report = Reporter::to(build_core::report::Format::Text, Box::new(std::io::sink()));
        // (before: read as an archive, and its file would have been texts/con.sarc.zs, a device on Windows)
        assert!(make(&romfs, &d.join("texts"), &cache, &report).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_file_or_string_is_left() {
        let strings = [Reword { msbt: "A.msbt", label: "L", edits: &[] }];
        let mut none = crate::sarc::tests::build(&[("B.msbt", b"x")]);
        assert_eq!(reword(&mut none, &strings, "t").unwrap().left[0].1, "no A.msbt");
        let m = msbt("Other", b"x\0");
        let mut other = crate::sarc::tests::build(&[("A.msbt", &m)]);
        assert_eq!(reword(&mut other, &strings, "t").unwrap().left[0].1, "no such string");
    }
}

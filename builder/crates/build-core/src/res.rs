//! Windows resource files (`.res`, what an exe's link takes) and icon files (`.ico`), as the builder writes them:
//! Longshot's icon and the manifest (build-compile), the game's name in the version strings (build-assets).
//!
//! A `.res` is a run of entries, each a header (data size, header size, type and name by number, flags, language)
//! and its data padded to 4, after an empty entry that says it is one. An `.ico` here holds PNG images, as Windows
//! Vista and later read them.

use crate::{Error, Result};

/// US English, the language every entry is in.
pub const US_ENGLISH: u16 = 0x0409;

/// `RT_ICON`, one image of an icon.
pub const RT_ICON: u16 = 3;
/// `RT_GROUP_ICON`, an icon's list of its images.
pub const RT_GROUP_ICON: u16 = 14;
/// `RT_VERSION`, the version strings.
pub const RT_VERSION: u16 = 16;
/// `RT_MANIFEST`.
pub const RT_MANIFEST: u16 = 24;

/// One entry: its type and name by number, its flags and language, the data 4-aligned.
pub fn entry(kind: u16, name: u16, data: &[u8], flags: u16, language: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + data.len() + 3);
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes());
    for v in [0xFFFF, kind, 0xFFFF, name] {
        out.extend(v.to_le_bytes());
    }
    out.extend(0u32.to_le_bytes()); // (the data version)
    out.extend(flags.to_le_bytes());
    out.extend(language.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // (the version, the characteristics)
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    out.resize(out.len() + (data.len().wrapping_neg() & 3), 0);
    out
}

/// The empty entry a `.res` starts with.
pub fn start(language: u16) -> Vec<u8> {
    entry(0, 0, b"", 0, language)
}

/// An icon's entries: each image an `RT_ICON` (1, 2, ...), then `RT_GROUP_ICON` 1 listing them (what the window
/// class loads, `host/display.c`). `images`: (size in pixels, PNG bytes).
pub fn icon_entries(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut res = Vec::new();
    let mut group = Vec::new();
    for v in [0u16, 1, images.len() as u16] {
        group.extend(v.to_le_bytes());
    }
    for (i, (s, data)) in images.iter().enumerate() {
        let b = (*s & 255) as u8; // (256 is 0)
        group.extend([b, b, 0, 0]);
        group.extend(1u16.to_le_bytes()); // (planes)
        group.extend(32u16.to_le_bytes()); // (bits per pixel)
        group.extend((data.len() as u32).to_le_bytes());
        group.extend((i as u16 + 1).to_le_bytes());
        res.extend(entry(RT_ICON, i as u16 + 1, data, 0x1010, US_ENGLISH));
    }
    res.extend(entry(RT_GROUP_ICON, 1, &group, 0x1030, US_ENGLISH));
    res
}

/// One `VS_VERSIONINFO` node: wLength, wValueLength (in WCHARs for text), wType, key, value, children, each 4-aligned.
fn node(key: &str, value: &[u8], text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut body: Vec<u8> = key.encode_utf16().flat_map(u16::to_le_bytes).collect();
    body.extend([0, 0]);
    let pad = |body: &mut Vec<u8>| body.resize(body.len() + ((6 + body.len()).wrapping_neg() & 3), 0);
    pad(&mut body);
    body.extend(value);
    for child in children {
        pad(&mut body);
        body.extend(child);
    }
    let count = if text { value.len() / 2 } else { value.len() };
    let mut out = Vec::with_capacity(6 + body.len());
    for v in [(6 + body.len()) as u16, count as u16, u16::from(text)] {
        out.extend(v.to_le_bytes());
    }
    out.extend(body);
    out
}

fn wide(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    v.extend([0, 0]);
    v
}

/// `RT_VERSION` 1: what Windows shows for the exe (Task Manager, the taskbar), `strings` (name, text) in order.
pub fn version_entry(strings: &[(&str, &str)]) -> Vec<u8> {
    let fixed: Vec<u8> = [0xFEEF_04BDu32, 0x0001_0000, 0, 0, 0, 0, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let table: Vec<Vec<u8>> = strings.iter().map(|(k, v)| node(k, &wide(v), true, &[])).collect();
    let translation = [US_ENGLISH.to_le_bytes(), 1200u16.to_le_bytes()].concat();
    let info = node(
        "VS_VERSION_INFO",
        &fixed,
        false,
        &[
            node("StringFileInfo", &[], true, &[node("040904b0", &[], true, &table)]),
            node("VarFileInfo", &[], true, &[node("Translation", &translation, false, &[])]),
        ],
    );
    entry(RT_VERSION, 1, &info, 0x0030, US_ENGLISH)
}

/// A `.res` without its icon entries (`RT_ICON`, `RT_GROUP_ICON`), the others byte for byte.
pub fn without_icons(data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let rec = data.get(at..at + 12).ok_or_else(|| Error::format("resource", "a header cut short"))?;
        let size = u32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]) as usize;
        let header = u32::from_le_bytes([rec[4], rec[5], rec[6], rec[7]]) as usize;
        let marker = u16::from_le_bytes([rec[8], rec[9]]);
        let kind = u16::from_le_bytes([rec[10], rec[11]]);
        let end = at + header + size + (size.wrapping_neg() & 3);
        if header < 12 || header & 3 != 0 || end > data.len() {
            return Err(Error::format("resource", "a size that is not one"));
        }
        if marker != 0xFFFF || !matches!(kind, RT_ICON | RT_GROUP_ICON) {
            out.extend_from_slice(&data[at..end]);
        }
        at = end;
    }
    Ok(out)
}

/// An `.ico` of PNG images, (size in pixels, PNG bytes).
pub fn ico_of(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [0u16, 1, images.len() as u16] {
        out.extend(v.to_le_bytes());
    }
    let mut offset = 6 + 16 * images.len();
    for (s, data) in images {
        let b = (*s & 255) as u8;
        out.extend([b, b, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in images {
        out.extend(data);
    }
    out
}

/// An `.ico`'s images when they are PNGs (as [`ico_of`] writes them, and `assets/longshot.ico` has them):
/// (size in pixels, PNG bytes).
pub fn ico_images(data: &[u8]) -> Result<Vec<(u32, Vec<u8>)>> {
    let bad = |d: &str| Error::format("icon", d);
    let u16_at = |i: usize| data.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |i: usize| data.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let (Some(0), Some(1), Some(n @ 1..)) = (u16_at(0), u16_at(2), u16_at(4)) else {
        return Err(bad("not an icon file"));
    };
    (0..usize::from(n))
        .map(|i| {
            let e = 6 + 16 * i;
            let w = match *data.get(e).ok_or_else(|| bad("cut short"))? {
                0 => 256,
                w => u32::from(w),
            };
            let (size, at) =
                (u32_at(e + 8).ok_or_else(|| bad("cut short"))?, u32_at(e + 12).ok_or_else(|| bad("cut short"))?);
            let png = data.get(at..at + size).ok_or_else(|| bad("an image past the file's end"))?;
            if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
                return Err(bad(&format!("an icon image that is not a PNG ({w} pixels)")));
            }
            Ok((w, png.to_vec()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kinds of a `.res`'s entries, checking each is whole.
    fn kinds(res: &[u8]) -> Vec<u16> {
        let (mut at, mut kinds) = (0, Vec::new());
        while at < res.len() {
            let size = u32::from_le_bytes(res[at..at + 4].try_into().unwrap()) as usize;
            let head = u32::from_le_bytes(res[at + 4..at + 8].try_into().unwrap()) as usize;
            kinds.push(u16::from_le_bytes([res[at + 10], res[at + 11]]));
            at += head + size + (size.wrapping_neg() & 3);
        }
        assert_eq!(at, res.len());
        kinds
    }

    #[test]
    fn an_icon_s_entries_come_out_and_the_rest_stay() {
        let png = b"\x89PNG\r\n\x1a\nxyz".to_vec();
        let res =
            [start(US_ENGLISH), icon_entries(&[(16, png.clone()), (256, png)]), version_entry(&[("A", "b")])].concat();
        assert_eq!(kinds(&res), [0, RT_ICON, RT_ICON, RT_GROUP_ICON, RT_VERSION]);
        assert_eq!(without_icons(&res).unwrap(), [start(US_ENGLISH), version_entry(&[("A", "b")])].concat());
    }

    #[test]
    fn an_ico_reads_back() {
        let images = vec![(16, b"\x89PNG\r\n\x1a\n1".to_vec()), (256, b"\x89PNG\r\n\x1a\n22".to_vec())];
        assert_eq!(ico_images(&ico_of(&images)).unwrap(), images);
        assert!(ico_images(b"\0\0\x02\0\x01\0").is_err());
        assert!(ico_images(&ico_of(&[(16, b"not a png".to_vec())])).is_err());
    }
}

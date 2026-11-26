//! SARC, the game's archive: a header, a file table (SFAT) and a name table (SFNT), then the files' data.
//!
//! Text archives (`Mals/*.sarc.zs`), the zstd dictionaries (`Pack/ZsDic.pack.zs`) and many shader packages are SARCs.
//! Either byte order is read (the byte-order mark at +6 says which); every file the game has is little-endian.

use build_core::{Error, Result};
use std::ops::Range;

/// One file of an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name, when the archive names its files.
    pub name: Option<String>,
    /// Where its bytes are in the archive.
    pub range: Range<usize>,
}

fn u16_at(data: &[u8], at: usize, big: bool) -> Option<u16> {
    let b: [u8; 2] = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(if big { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) })
}

fn u32_at(data: &[u8], at: usize, big: bool) -> Option<u32> {
    let b: [u8; 4] = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(if big { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
}

/// Whether `data` is a SARC.
pub fn is_sarc(data: &[u8]) -> bool {
    data.starts_with(b"SARC")
}

/// The files of the SARC `data`, in the table's order. `what` names it in an error.
pub fn entries(data: &[u8], what: &str) -> Result<Vec<Entry>> {
    let bad = |detail: &str| Error::format(what, detail);
    if !is_sarc(data) {
        return Err(bad("not a SARC"));
    }
    let big = data.get(6..8) == Some(&[0xFE, 0xFF]);
    let header = usize::from(u16_at(data, 4, big).ok_or_else(|| bad("cut short"))?);
    let base = u32_at(data, 12, big).ok_or_else(|| bad("cut short"))? as usize;
    if data.get(header..header + 4) != Some(b"SFAT") {
        return Err(bad("a SARC without its file table"));
    }
    let count = usize::from(u16_at(data, header + 6, big).ok_or_else(|| bad("cut short"))?);
    let names = header + 12 + 16 * count + 8; // (after SFNT's header)
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let rec = header + 12 + 16 * i;
        let field = |k: usize| u32_at(data, rec + 4 * k, big).ok_or_else(|| bad("its file table is cut short"));
        let (attr, start, end) = (field(1)?, field(2)? as usize, field(3)? as usize);
        if start > end || base + end > data.len() {
            return Err(bad("a file past the archive's end"));
        }
        let name = if attr & 0x0100_0000 != 0 {
            let at = names + 4 * (attr & 0xFFFF) as usize;
            let rest = data.get(at..).ok_or_else(|| bad("a name past the archive's end"))?;
            let len = rest.iter().position(|&b| b == 0).ok_or_else(|| bad("a name without its end"))?;
            Some(String::from_utf8_lossy(&rest[..len]).into_owned())
        } else {
            None
        };
        out.push(Entry { name, range: base + start..base + end });
    }
    Ok(out)
}

/// The file named `name` in the SARC `data`.
pub fn find<'a>(data: &'a [u8], name: &str, what: &str) -> Result<Option<&'a [u8]>> {
    Ok(entries(data, what)?.into_iter().find(|e| e.name.as_deref() == Some(name)).map(|e| &data[e.range]))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian SARC of `files` (name, bytes), as the game's are laid out.
    pub(crate) fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut names = Vec::new();
        let mut offsets = Vec::new();
        for (name, _) in files {
            offsets.push(names.len() / 4);
            names.extend_from_slice(name.as_bytes());
            names.push(0);
            while names.len() % 4 != 0 {
                names.push(0);
            }
        }
        let header = 0x14;
        let sfat = 12 + 16 * files.len();
        let base = header + sfat + 8 + names.len();
        let mut data_part = Vec::new();
        let mut table = Vec::new();
        for (k, (_, bytes)) in files.iter().enumerate() {
            let start = data_part.len() as u32;
            data_part.extend_from_slice(bytes);
            table.extend_from_slice(&0u32.to_le_bytes()); // (the name's hash: not read)
            table.extend_from_slice(&(0x0100_0000 | offsets[k] as u32).to_le_bytes());
            table.extend_from_slice(&start.to_le_bytes());
            table.extend_from_slice(&(data_part.len() as u32).to_le_bytes());
        }
        let mut out = b"SARC".to_vec();
        out.extend_from_slice(&(header as u16).to_le_bytes());
        out.extend_from_slice(&[0xFF, 0xFE]);
        out.extend_from_slice(&((base + data_part.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(base as u32).to_le_bytes());
        out.extend_from_slice(&[0x00, 0x01, 0, 0]);
        out.extend_from_slice(b"SFAT");
        out.extend_from_slice(&12u16.to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&0x65u32.to_le_bytes());
        out.extend_from_slice(&table);
        out.extend_from_slice(b"SFNT");
        out.extend_from_slice(&8u16.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&names);
        out.extend_from_slice(&data_part);
        out
    }

    #[test]
    fn files_by_name() {
        let sarc = build(&[("a.txt", b"one"), ("dir/b.bin", b"twotwo")]);
        let e = entries(&sarc, "test").unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].name.as_deref(), Some("dir/b.bin"));
        assert_eq!(find(&sarc, "a.txt", "test").unwrap(), Some(&b"one"[..]));
        assert_eq!(find(&sarc, "c", "test").unwrap(), None);
        assert!(entries(b"SARX", "test").is_err());
        assert!(entries(&sarc[..30], "test").is_err());
    }
}

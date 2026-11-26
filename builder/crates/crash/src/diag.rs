//! The diagnostics bundle: one zip of what helps with a problem, ready to attach to an issue, sent nowhere. The logs
//! and every other text in it redacted ([`crate::redact`]: the home folder and the user's name out), the build's record
//! and the profile's settings (never the saves: a file in a `saves` folder, or a `.sav`/`.bin`, is never taken), and
//! this PC's system. `longshot-builder diagnostics --out <zip>` and the error and crash windows' Save diagnostics make
//! it.

use crate::redact::Who;
use std::io::Write;
use std::path::{Path, PathBuf};

/// What goes in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bundle {
    /// Files, each under its own name in the zip (the logs, the build's record, profile.json).
    pub files: Vec<PathBuf>,
}

impl Bundle {
    /// A file added when it is there and may go ([`allowed`]).
    pub fn add(&mut self, path: impl Into<PathBuf>) {
        let p = path.into();
        if p.is_file() && allowed(&p) && !self.files.contains(&p) {
            self.files.push(p);
        }
    }
}

/// Whether a file may go into a bundle: text the builder or the game writes (.txt, .log, .json, .ini), never a save
/// (in a `saves` folder) nor anything else.
pub fn allowed(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let in_saves = p.components().any(|c| c.as_os_str().to_string_lossy().eq_ignore_ascii_case("saves"));
    !in_saves && ["txt", "log", "json", "ini"].contains(&ext.as_str())
}

fn crc32(b: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new();
    h.update(b);
    h.finalize()
}

/// A zip of `entries` (name, bytes), deflated: its bytes.
pub fn zip(entries: &[(String, Vec<u8>)]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    // (a fixed time, 1980-01-01: the same inputs make the same bytes)
    let (time, date) = (0u16, 0x21u16);
    for (name, data) in entries {
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(data)?;
        let packed = z.finish()?;
        let crc = crc32(data);
        let at = out.len() as u32;
        let head = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend(sig.to_le_bytes());
            if central {
                h.extend(20u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes()); // version needed
            h.extend(0x0800u16.to_le_bytes()); // UTF-8 names
            h.extend(8u16.to_le_bytes()); // deflate
            h.extend(time.to_le_bytes());
            h.extend(date.to_le_bytes());
            h.extend(crc.to_le_bytes());
            h.extend((packed.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central {
                h.extend([0u8; 8]); // comment, disk, internal attributes
                h.extend(0u32.to_le_bytes()); // external attributes
                h.extend(at.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        out.extend(head(0x0403_4b50, false));
        out.extend(&packed);
        central.extend(head(0x0201_4b50, true));
    }
    let (cd_at, cd_len) = (out.len() as u32, central.len() as u32);
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend(cd_len.to_le_bytes());
    out.extend(cd_at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    Ok(out)
}

/// A line as the bundle keeps it: the game's own redaction ([`Who::redact`]), then the build report's, which also knows
/// Windows' 8.3 short names (`ANASMI~1`) and a home folder written in other forms ([`crate::build_report::scrub`]): the
/// bundle is attached to public issues.
fn scrub(who: &Who, line: &str) -> String {
    crate::build_report::scrub(&who.redact(line), &who.home, &[who.user.as_str()])
}

/// The bundle written to `out`: each file redacted, under a name of its own, and `system.txt` (this PC).
pub fn write(b: &Bundle, out: &Path, who: &Who) -> std::io::Result<()> {
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for f in &b.files {
        let Some(text) = crate::store::read_text(f, 64 << 20) else { continue };
        let base = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file.txt".into());
        let mut name = base.clone();
        for n in 2.. {
            if !entries.iter().any(|(e, _)| *e == name) {
                break;
            }
            name = format!("{n}-{base}");
        }
        let clean: String = text.split_inclusive('\n').map(|l| scrub(who, l)).collect();
        entries.push((name, clean.into_bytes()));
    }
    let system =
        format!("{}\nbuilder: {} {}\n", crate::report::system_text(), std::env::consts::OS, std::env::consts::ARCH);
    entries.push(("system.txt".into(), scrub(who, &system).into_bytes()));
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, zip(&entries)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_never_go() {
        assert!(allowed(Path::new("/p/logs/longshot_log.txt")));
        assert!(allowed(Path::new("/p/profile.json")));
        assert!(!allowed(Path::new("/p/saves/profile.json")), "nothing of a saves folder");
        assert!(!allowed(Path::new("/p/zelda.sav")));
        assert!(!allowed(Path::new("/p/game.nup")));
    }

    #[test]
    fn the_zip_reads_back() {
        let z = zip(&[("a.txt".into(), b"hello hello hello".to_vec()), ("b.json".into(), b"{}".to_vec())]).unwrap();
        assert_eq!(&z[..4], b"PK\x03\x04");
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([z[z.len() - 12], z[z.len() - 11]]), 2);
        // (the first entry inflates back)
        let n = u16::from_le_bytes([z[26], z[27]]) as usize;
        let size = u32::from_le_bytes(z[18..22].try_into().unwrap()) as usize;
        let data = &z[30 + n..30 + n + size];
        let mut d = flate2::read::DeflateDecoder::new(data);
        let mut s = String::new();
        std::io::Read::read_to_string(&mut d, &mut s).unwrap();
        assert_eq!(s, "hello hello hello");
    }
}

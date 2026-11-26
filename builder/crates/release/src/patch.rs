//! A delta applied: `zstd -d --long=30 --patch-from=<base> <delta>`, as the zstd program does it, with the base as
//! the frame's prefix (a raw-content dictionary: its bytes before the frame's own, the repeat offsets zstd's defaults).
//! ruzstd decodes it (pure Rust, the zstd the builder already has: its pack and the game's `.zs` files), so it works the
//! same on Windows and Linux with no zstd program (the Longshot Launcher's App/ZstdPatcher.cs, ported).
//!
//! A delta is one zstd frame, of no dictionary id, whose window is at most 2^[`WINDOW_LOG`] (dev/release_deltas.rs's
//! `LONG`, the launcher's `DeltaWindowLog`): anything else is refused before it is decoded. The base and the result
//! are held in memory while it decodes (about three times the builder's size at most).

use crate::Cancel;
use ruzstd::decoding::{BlockDecodingStrategy, Dictionary, FrameDecoder};
use std::io::{Read, Write};
use std::path::Path;

/// The deltas' zstd window, as a power of two: the most a delta may ask the patcher to hold back.
pub const WINDOW_LOG: u32 = 30;
/// The dictionary id the base is held under (the frame names none; it is forced).
const PREFIX: u32 = 0x4c53_4250;

/// The window a zstd frame's header asks for, and whether it names a dictionary; Err for a header that is not one.
fn frame_window(head: &[u8]) -> Result<(u64, bool), String> {
    if head.get(..4) != Some(&[0x28, 0xb5, 0x2f, 0xfd][..]) {
        return Err("not a zstd frame".into());
    }
    let fhd = *head.get(4).ok_or("a frame header cut short")?;
    let single = fhd & 0x20 != 0;
    let dict_bytes = [0usize, 1, 2, 4][usize::from(fhd & 3)];
    let fcs_bytes = match fhd >> 6 {
        0 => usize::from(single),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let mut at = 5;
    let window = if single {
        None
    } else {
        let wd = *head.get(at).ok_or("a frame header cut short")?;
        at += 1;
        let log = 10 + u64::from(wd >> 3);
        let base = 1u64 << log.min(63);
        Some(base + (base / 8) * u64::from(wd & 7))
    };
    let dict = head.get(at..at + dict_bytes).ok_or("a frame header cut short")?.iter().any(|&b| b != 0);
    at += dict_bytes;
    let window = match window {
        Some(w) => w,
        None => {
            let b = head.get(at..at + fcs_bytes).ok_or("a frame header cut short")?;
            let mut n = 0u64;
            for (i, &x) in b.iter().enumerate() {
                n |= u64::from(x) << (8 * i);
            }
            if fcs_bytes == 2 { n + 256 } else { n }
        }
    };
    Ok((window, dict))
}

/// The file `out` made from `base` (the earlier full file's bytes) and the delta at `delta`; Err why it is not a
/// delta of that base (or cancelled). `out` is written whole or left partial for the caller to delete.
pub fn apply(base: Vec<u8>, delta: &Path, out: &Path, cancel: &Cancel) -> Result<(), String> {
    let mut file = std::fs::File::open(delta).map_err(|e| format!("{}: {e}", delta.display()))?;
    let mut head = [0u8; 18];
    let n = file.read(&mut head).map_err(|e| e.to_string())?;
    let (window, dict) = frame_window(&head[..n])?;
    if dict {
        return Err("the delta names a dictionary".into());
    }
    if window > 1u64 << WINDOW_LOG {
        return Err(format!("the delta's window is larger than 2^{WINDOW_LOG}"));
    }
    drop(file);
    let mut src = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(delta).map_err(|e| e.to_string())?);
    let mut dec = FrameDecoder::new();
    dec.add_dict(Dictionary {
        id: PREFIX,
        fse: Default::default(),
        huf: Default::default(),
        dict_content: base,
        offset_hist: [1, 4, 8],
    })
    .map_err(|e| format!("zstd: {e}"))?;
    dec.reset(&mut src).map_err(|e| format!("zstd: {e}"))?;
    dec.force_dict(PREFIX).map_err(|e| format!("zstd: {e}"))?;
    let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(out).map_err(|e| e.to_string())?);
    while !dec.is_finished() {
        if cancel.is_set() {
            return Err("cancelled".into());
        }
        dec.decode_blocks(&mut src, BlockDecodingStrategy::UptoBytes(4 << 20)).map_err(|e| format!("zstd: {e}"))?;
        dec.collect_to_writer(&mut w).map_err(|e| e.to_string())?;
    }
    dec.collect_to_writer(&mut w).map_err(|e| e.to_string())?;
    if let Some(sum) = dec.get_checksum_from_data()
        && dec.get_calculated_checksum().is_some_and(|c| c != sum)
    {
        return Err("the delta's checksum is not its content's".into());
    }
    let mut rest = [0u8; 1];
    if src.read(&mut rest).map_err(|e| e.to_string())? != 0 {
        return Err("the delta has more than one zstd frame".into());
    }
    w.flush().map_err(|e| e.to_string())?;
    w.into_inner().map_err(|e| e.to_string())?.sync_all().map_err(|e| e.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The fixtures' base and target: made here, the same bytes each time (a pseudo-random stream, the target the
    /// base with a few changes, a part moved and more at the end).
    pub fn fixture_pair() -> (Vec<u8>, Vec<u8>) {
        let mut x = 0x1234_5678u32;
        let base: Vec<u8> = (0..200_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 24) as u8
            })
            .collect();
        let mut target = base.clone();
        for i in (0..target.len()).step_by(9973) {
            target[i] ^= 0x5a;
        }
        target.extend_from_slice(&base[1000..30_000]);
        target.extend_from_slice(b"the end of the new release");
        (base, target)
    }

    /// `zstd -19 --long=30 --patch-from=base target`, made by the zstd program from [`fixture_pair`]:
    /// tests/fixtures/delta.zst-patch.
    pub const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/delta.zst-patch");

    fn scratch(name: &str) -> std::path::PathBuf {
        crate::tests::scratch(&format!("patch-{name}"))
    }

    #[test]
    fn a_patch_from_delta_gives_the_target_back() {
        let d = scratch("ok");
        let (base, target) = fixture_pair();
        std::fs::write(d.join("delta"), FIXTURE).unwrap();
        apply(base.clone(), &d.join("delta"), &d.join("out"), &Cancel::default()).unwrap();
        assert!(std::fs::read(d.join("out")).unwrap() == target, "the target's bytes");
        // (another base: refused, or not the target)
        let mut other = base.clone();
        other[5000] ^= 1;
        let r = apply(other, &d.join("delta"), &d.join("out"), &Cancel::default());
        assert!(r.is_err() || std::fs::read(d.join("out")).unwrap() != target);
        // (cut short, two frames, not zstd)
        std::fs::write(d.join("cut"), &FIXTURE[..FIXTURE.len() / 2]).unwrap();
        assert!(apply(base.clone(), &d.join("cut"), &d.join("out"), &Cancel::default()).is_err());
        std::fs::write(d.join("two"), [FIXTURE, FIXTURE].concat()).unwrap();
        assert!(
            apply(base.clone(), &d.join("two"), &d.join("out"), &Cancel::default())
                .unwrap_err()
                .contains("more than one")
        );
        std::fs::write(d.join("no"), b"hello").unwrap();
        assert!(apply(base, &d.join("no"), &d.join("out"), &Cancel::default()).unwrap_err().contains("not a zstd"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_window_past_2_to_the_30_is_refused() {
        // (FHD: not single segment, no dictionary; window descriptor exponent 21: 2^31)
        assert_eq!(frame_window(&[0x28, 0xb5, 0x2f, 0xfd, 0x00, 21 << 3]).unwrap(), (1 << 31, false));
        assert_eq!(frame_window(&[0x28, 0xb5, 0x2f, 0xfd, 0x00, 20 << 3]).unwrap(), (1 << 30, false));
        assert!(frame_window(&[0x28, 0xb5, 0x2f, 0xfd, 0x01, 20 << 3, 7]).unwrap().1, "a dictionary id");
        let d = scratch("window");
        std::fs::write(d.join("big"), [0x28, 0xb5, 0x2f, 0xfd, 0x00, 21 << 3, 0, 0, 0]).unwrap();
        let e = apply(vec![1, 2, 3], &d.join("big"), &d.join("out"), &Cancel::default()).unwrap_err();
        assert!(e.contains("window"), "{e}");
        std::fs::remove_dir_all(&d).unwrap();
    }
}

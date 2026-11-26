//! What the game folder keeps of the two game files the prompts write over (the textures, the layouts): not a copy
//! (the host reads them from the romfs as they stream: `host/prompts_base.h`), only the game's bytes under every run
//! any face may write, and the few bytes the host knows the file by.
//!
//! ```text
//! prompts/<Common.bntx | Common.blarc>.base: "HPBS", u32 version (1), u64 the file's size unpacked,
//!     u32 samples, u64 each sample's offset, u32 runs, then each: u32 offset, u32 length, the game's bytes there
//! ```
//!
//! The runs are sorted, merged where they touch or overlap: any range inside the ranges given is inside one run.

/// The base of `file` holding its bytes under `ranges` (offset, length) and the samples' (each `SAMPLE` bytes).
pub fn base(file: &[u8], ranges: &[(usize, usize)], samples: &[u64]) -> Vec<u8> {
    let mut all: Vec<(usize, usize)> = ranges.iter().map(|&(o, n)| (o, o.saturating_add(n).min(file.len()))).collect();
    all.extend(samples.iter().map(|&s| (s as usize, (s as usize).saturating_add(SAMPLE).min(file.len()))));
    all.retain(|r| r.0 < r.1);
    all.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (a, z) in all {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(z),
            _ => merged.push((a, z)),
        }
    }
    let mut out = b"HPBS".to_vec();
    out.extend(1u32.to_le_bytes());
    out.extend((file.len() as u64).to_le_bytes());
    out.extend((samples.len() as u32).to_le_bytes());
    for s in samples {
        out.extend(s.to_le_bytes());
    }
    out.extend((merged.len() as u32).to_le_bytes());
    for (a, z) in merged {
        out.extend((a as u32).to_le_bytes());
        out.extend(((z - a) as u32).to_le_bytes());
        out.extend(&file[a..z]);
    }
    out
}

/// The bytes a sample holds (`host/prompts.c SAMPLE_LEN`).
pub const SAMPLE: usize = 64;

fn u64_at(d: &[u8], at: usize) -> Option<usize> {
    Some(u64::from_le_bytes(d.get(at..at.checked_add(8)?)?.try_into().ok()?) as usize)
}

/// The base of the textures (a BNTX) under `ranges`: with its header (0x38 bytes), its BRTD block's (16), and three
/// samples of its texture data (at a quarter, a half and three quarters in, moved on by 4 KB while any range is
/// there), by which the host knows the game's memory pool holding it.
pub fn textures_base(bntx: &[u8], ranges: &[(usize, usize)]) -> Vec<u8> {
    let mut r = ranges.to_vec();
    r.push((0, 0x38));
    let mut samples = Vec::new();
    if let Some(brtd) = u64_at(bntx, 0x30)
        && bntx.get(brtd..brtd.saturating_add(4)) == Some(b"BRTD")
        && let Some(size) = u64_at(bntx, brtd + 8)
    {
        r.push((brtd, 16));
        // (the block's size as it says, within the file: a sample is in the file's bytes)
        let (data_off, data_len) = (brtd + 16, size.saturating_sub(16).min(bntx.len().saturating_sub(brtd + 16)));
        let hit = |at: usize| ranges.iter().any(|&(o, n)| at < o.saturating_add(n) && o < at + SAMPLE);
        for s in 1..=3 {
            let mut at = data_off + data_len / 4 * s;
            while hit(at) && at + 2 * SAMPLE < data_off + data_len {
                at += 4096;
            }
            samples.push(at as u64);
        }
    }
    base(bntx, &r, &samples)
}

/// A file's runs' ranges (`textures.bin`, `ring.bin`: a magic, u32 count, then each entry's 4 bytes, u32 runs and the
/// runs).
pub fn run_ranges(bin: &[u8]) -> Vec<(usize, usize)> {
    let u32_at = |at: usize| {
        bin.get(at..at.checked_add(4)?).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
    };
    let mut out = Vec::new();
    let Some(count) = u32_at(4) else { return out };
    let mut at = 8;
    for _ in 0..count {
        let Some(nr) = u32_at(at + 4) else { break };
        at += 8;
        for _ in 0..nr {
            let (Some(off), Some(len)) = (u32_at(at), u32_at(at + 4)) else { return out };
            out.push((off, len));
            at += 8 + len;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_into_runs_of_the_game_s_bytes() {
        let file: Vec<u8> = (0..200u8).collect();
        let b = base(&file, &[(10, 5), (12, 10), (22, 3), (100, 4)], &[150]);
        assert_eq!(&b[..4], b"HPBS");
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 200);
        assert_eq!(u32::from_le_bytes(b[16..20].try_into().unwrap()), 1);
        let at = 28;
        assert_eq!(u32::from_le_bytes(b[at..at + 4].try_into().unwrap()), 3, "10..25, 100..104, the sample's 150..200");
        assert_eq!(u32::from_le_bytes(b[at + 4..at + 8].try_into().unwrap()), 10);
        assert_eq!(u32::from_le_bytes(b[at + 8..at + 12].try_into().unwrap()), 15);
        assert_eq!(&b[at + 12..at + 27], &file[10..25]);
    }

    /// host/prompts_base.c, built alone with tests/prompts_base_test.c, run on a synthetic file, its runs and its base.
    #[test]
    fn the_host_patches_the_stream() {
        let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        if std::process::Command::new(&cc).arg("--version").output().is_err() {
            eprintln!("the_host_patches_the_stream: no C compiler ({cc}): left out");
            return;
        }
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let d = std::env::temp_dir().join(format!("build-assets-host-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // (a file of three raw blocks and a bit; runs across a block's end, one over another, at the very end)
        let file: Vec<u8> = (0..300_001u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        let runs: Vec<(usize, Vec<u8>)> = vec![
            (131_000, vec![1; 2000]),
            (131_500, vec![2; 10]),
            (5, vec![3; 3]),
            (299_990, vec![4; 11]),
            (200_000, vec![5; 70_000]),
        ];
        let mut bin = b"HPTX".to_vec();
        bin.extend((runs.len() as u32).to_le_bytes());
        for (off, data) in &runs {
            bin.extend([0xff, 0, 0, 0]);
            bin.extend(1u32.to_le_bytes());
            bin.extend((*off as u32).to_le_bytes());
            bin.extend((data.len() as u32).to_le_bytes());
            bin.extend(data);
        }
        let ranges = run_ranges(&bin);
        std::fs::write(d.join("base"), base(&file, &ranges, &[77])).unwrap();
        std::fs::write(d.join("game"), &file).unwrap();
        std::fs::write(d.join("textures.bin"), &bin).unwrap();
        let exe = d.join("prompts_base_test");
        let built = std::process::Command::new(&cc)
            .args(["-std=gnu11", "-O1", "-Wall", "-Wextra", "-Werror"])
            .arg(repo.join("tests/prompts_base_test.c"))
            .arg(repo.join("host/prompts_base.c"))
            .arg("-o")
            .arg(&exe)
            .status()
            .unwrap();
        assert!(built.success(), "the host's base reader builds");
        let run = |game: &str| {
            std::process::Command::new(&exe)
                .args([d.join("base"), d.join(game), d.join("textures.bin"), d.join("out")])
                .status()
                .unwrap()
                .code()
        };
        assert_eq!(run("game"), Some(0));
        let mut want = file.clone();
        for (off, data) in &runs {
            want[*off..*off + data.len()].copy_from_slice(data); // (in order: a later one over an earlier)
        }
        let out = std::fs::read(d.join("out")).unwrap();
        assert_eq!(crate::zs::Decoder::plain().decompress(&out, "out").unwrap(), want);
        // (another version of the file: its bytes under a run not the base's, or another size)
        let mut other = file.clone();
        other[131_700] ^= 1;
        std::fs::write(d.join("other"), &other).unwrap();
        assert_eq!(run("other"), Some(1), "the base's bytes are checked against the file's as it streams");
        std::fs::write(d.join("short"), &file[..file.len() - 1]).unwrap();
        assert_eq!(run("short"), Some(1), "a file cut short is refused");
        // (a base whose runs touch: refused, as the builder merges them; a range across the two would be in neither)
        let mut touching = b"HPBS".to_vec();
        touching.extend(1u32.to_le_bytes());
        touching.extend((file.len() as u64).to_le_bytes());
        touching.extend(0u32.to_le_bytes());
        touching.extend(2u32.to_le_bytes());
        for (a, z) in [(0usize, 16usize), (16, 32)] {
            touching.extend((a as u32).to_le_bytes());
            touching.extend(((z - a) as u32).to_le_bytes());
            touching.extend(&file[a..z]);
        }
        std::fs::write(d.join("base"), touching).unwrap();
        assert_eq!(run("game"), Some(3), "a base whose runs touch is refused");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_bin_s_ranges() {
        let mut bin = b"HPTX".to_vec();
        bin.extend(1u32.to_le_bytes());
        bin.extend([0xff, 0, 0, 0]);
        bin.extend(2u32.to_le_bytes());
        for (off, data) in [(64u32, &b"ab"[..]), (900, b"xyz")] {
            bin.extend(off.to_le_bytes());
            bin.extend((data.len() as u32).to_le_bytes());
            bin.extend(data);
        }
        assert_eq!(run_ranges(&bin), [(64, 2), (900, 3)]);
    }
}

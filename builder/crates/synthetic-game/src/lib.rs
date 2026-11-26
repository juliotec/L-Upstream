//! The synthetic game: what the builder's end-to-end test (`tests/end_to_end.rs`) builds and runs in place of the
//! game, which no test may read. Everything it is made of is in this crate, written by hand:
//!
//! - `game/main.S`, a small AArch64 program (integer, floating-point and memory operations, indirect calls, an SVC, two
//!   RomFS files read through a host import, a function big enough to be a unit of its own), and `game/module.ld`, its
//!   module's layout. The test assembles and links them with the pinned clang at test time;
//! - [`nso_from_elf`]: the linked module as an NSO, the way the console's code modules are;
//! - [`npdm`]: its `main.npdm`;
//! - [`romfs_files`]: its RomFS, with the files the read stage asks every RomFS for;
//! - [`expected_result`]: what `nnMain` returns, computed here, for the test to compare with the run's;
//! - `game/host/`: the test's host, which the lifted program is linked with (the real runtime, `builder/rt`, and the real
//!   `host/romfs.c` and `host/romfs_index.c` beside it).

/// The synthetic game's build id (main's, in its NSO header): not the supported build's, so the test builds it through
/// the read stage's allowlist of builds (`build_driver::read::read_exefs_of`), never by weakening the player's check.
pub const BUILD_ID: [u8; 32] = *b"\x5e\x1f\x7e\x57synthetic game, not the game";

/// [`BUILD_ID`] as the read stage writes a build id: hex, its trailing zero bytes left off.
pub fn build_id_hex() -> String {
    let kept = BUILD_ID.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
    BUILD_ID[..kept].iter().map(|b| format!("{b:02x}")).collect()
}

/// The synthetic game's title id (made up).
pub const TITLE_ID: u64 = 0x0100_5e1f_7e57_0000;

/// The RomFS file the program reads as the .nup stores it.
pub const PLAIN_FILE: (&str, &[u8]) =
    ("Synthetic/plain.txt", b"Hello from the synthetic game's RomFS: read in place from its .nup.\n");

/// The RomFS file the program reads that the .nup stores as one zstd frame: its bytes.
pub fn packed_file() -> Vec<u8> {
    (0..300u32).map(|i| (i * 7 + i / 13) as u8).collect()
}

/// The RomFS file's path that [`packed_file`] is at.
pub const PACKED_PATH: &str = "Synthetic/packed.bin";

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| format!("no field at {at:#x}"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap_or_default()))
        .ok_or_else(|| format!("no field at {at:#x}"))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64, String> {
    b.get(at..at + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap_or_default()))
        .ok_or_else(|| format!("no field at {at:#x}"))
}

/// A loadable segment of an ELF file: its file offset, its address, its size in the file and in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Load {
    offset: usize,
    addr: usize,
    file: usize,
    memory: usize,
}

/// An AArch64 ELF module linked by `game/module.ld` (three loadable segments from address 0: code, read-only data,
/// data and bss) as an NSO: the three segments stored as they are (no compression, no hashes to check), the bss's size
/// and `build_id` in the header.
pub fn nso_from_elf(elf: &[u8], build_id: &[u8; 32]) -> Result<Vec<u8>, String> {
    if elf.get(..4) != Some(b"\x7fELF") || elf.get(4) != Some(&2) || elf.get(5) != Some(&1) {
        return Err("not a 64-bit little-endian ELF file".into());
    }
    if u16_at(elf, 0x12)? != 183 {
        return Err("not an AArch64 module".into());
    }
    let (phoff, phentsize, phnum) = (u64_at(elf, 0x20)? as usize, u16_at(elf, 0x36)? as usize, u16_at(elf, 0x38)?);
    let mut loads = Vec::new();
    for i in 0..usize::from(phnum) {
        let at = phoff + i * phentsize;
        if u32_at(elf, at)? != 1 {
            continue; // (PT_LOAD only)
        }
        loads.push(Load {
            offset: u64_at(elf, at + 8)? as usize,
            addr: u64_at(elf, at + 0x10)? as usize,
            file: u64_at(elf, at + 0x20)? as usize,
            memory: u64_at(elf, at + 0x28)? as usize,
        });
    }
    let [text, rodata, data] = loads[..] else {
        return Err(format!("{} loadable segments: module.ld makes three", loads.len()));
    };
    if text.addr != 0 || rodata.addr < text.addr + text.memory || data.addr < rodata.addr + rodata.memory {
        return Err("the segments are not in order from address 0".into());
    }
    if text.memory != text.file || rodata.memory != rodata.file {
        return Err("only the data segment may have a bss".into());
    }
    let mut out = vec![0u8; 0x100];
    out[..4].copy_from_slice(b"NSO0");
    let mut body = Vec::new();
    for (i, s) in [text, rodata, data].iter().enumerate() {
        let bytes = elf.get(s.offset..s.offset + s.file).ok_or("a segment past the file's end")?;
        let field = |k: usize| 0x10 + 0x10 * i + 4 * k;
        out[field(0)..field(0) + 4].copy_from_slice(&((0x100 + body.len()) as u32).to_le_bytes());
        out[field(1)..field(1) + 4].copy_from_slice(&(s.addr as u32).to_le_bytes());
        out[field(2)..field(2) + 4].copy_from_slice(&(s.file as u32).to_le_bytes());
        out[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(s.file as u32).to_le_bytes());
        body.extend_from_slice(bytes);
    }
    // (the data segment's fourth word: the bss's size)
    out[0x3C..0x40].copy_from_slice(&((data.memory - data.file) as u32).to_le_bytes());
    out[0x40..0x60].copy_from_slice(build_id);
    out.extend_from_slice(&body);
    Ok(out)
}

/// The synthetic game's `main.npdm`: a META header with the main thread's priority (44) and stack size (1 MB), no
/// ACI0 (program id 0).
pub fn npdm() -> Vec<u8> {
    let mut b = vec![0u8; 0x80];
    b[..4].copy_from_slice(b"META");
    b[0xE] = 44;
    b[0x1C..0x20].copy_from_slice(&0x10_0000u32.to_le_bytes());
    b
}

/// The synthetic game's RomFS: (path, bytes, codec: 0 stored as it is, 1 one zstd frame). The two files the program
/// reads, and stand-ins for those the read stage asks of every RomFS (game-files' `checks::romfs_missing`): a zstd
/// frame for each `.zs` file (the read stage reads their first bytes to find a damaged dump) and a file in
/// System/Resource.
pub fn romfs_files(zstd_frame: impl Fn(&[u8]) -> Vec<u8>) -> Vec<(String, Vec<u8>, u8)> {
    let zs = |text: &str| zstd_frame(text.as_bytes());
    vec![
        (PLAIN_FILE.0.into(), PLAIN_FILE.1.to_vec(), 0),
        (PACKED_PATH.into(), packed_file(), 1),
        ("Pack/ZsDic.pack.zs".into(), zs("(a stand-in: the synthetic game has no dictionaries)"), 0),
        ("System/HeapSize/Default.Nin_Ounce_NVN2.HeapDef.byml.zs".into(), zs("(a stand-in)"), 0),
        ("System/Resource/synthetic.txt".into(), b"(a stand-in)\n".to_vec(), 0),
    ]
}

/// What `game/main.S`'s nnMain returns with `svc_arg` (its SVC_ARG) and `big_adds` (its BIG_ADDS), the host answering
/// as `game/host/synthetic_host.c` does: the same steps, in Rust.
pub fn expected_result(svc_arg: u64, big_adds: u64) -> u64 {
    // integers
    let int_ops = |a: u64| {
        let mut acc = 0u64;
        for i in 1..=20u64 {
            acc ^= i.wrapping_mul(a) >> 1;
            acc = acc.wrapping_add(i << 4);
        }
        let (q, r) = (acc / 7, acc % 7);
        let v = q.wrapping_mul(r).wrapping_add(acc).wrapping_add(if r > 3 { 0x55 } else { 0 });
        v.rotate_right(13)
    };
    // floating point
    let float_ops = |n: i64| {
        let d = ((n as f64) * 2.5 + 0.75).sqrt() * 1000.0;
        let s = (d as f32) * 0.5f32;
        (d as i64 as u64).wrapping_add(i64::from(s as i32) as u64)
    };
    // memory
    let squares: u64 = (0..16u64).map(|i| i * i).sum();
    let counter = 0x1122_3344u64 + 1;
    let memory = squares + ((counter >> 8) & 0xff) + counter;
    // a RomFS file's checksum plus its size << 32
    let file = |bytes: &[u8]| {
        let sum = bytes.iter().enumerate().fold(0u64, |s, (i, &b)| s.wrapping_add(u64::from(b) * (i as u64 + 1)));
        sum.wrapping_add((bytes.len() as u64) << 32)
    };
    let mut r = int_ops(0x1234);
    r = r.wrapping_add(float_ops(10));
    r = r.wrapping_add(memory);
    r = r.wrapping_add(7 * 3).wrapping_add(9 * 9);
    r = r.wrapping_add(svc_arg * 1000 + 1);
    r = r.wrapping_add(file(PLAIN_FILE.1));
    r ^= file(&packed_file()).rotate_right(8);
    r.wrapping_add(3 * big_adds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_s_segments_become_an_nso_s() {
        // (a hand-made ELF header and three program headers: code at 0, rodata at 0x1000, data at 0x2000 with a bss)
        let mut elf = vec![0u8; 0x300];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[0x12..0x14].copy_from_slice(&183u16.to_le_bytes());
        elf[0x20..0x28].copy_from_slice(&0x40u64.to_le_bytes());
        elf[0x36..0x38].copy_from_slice(&0x38u16.to_le_bytes());
        elf[0x38..0x3A].copy_from_slice(&3u16.to_le_bytes());
        for (i, (offset, addr, file, memory)) in
            [(0x100u64, 0u64, 0x10u64, 0x10u64), (0x200, 0x1000, 8, 8), (0x280, 0x2000, 8, 0x40)]
                .into_iter()
                .enumerate()
        {
            let at = 0x40 + 0x38 * i;
            elf[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
            for (k, v) in [(8, offset), (0x10, addr), (0x20, file), (0x28, memory)] {
                elf[at + k..at + k + 8].copy_from_slice(&v.to_le_bytes());
            }
            elf[offset as usize] = 0xA0 + i as u8;
        }
        let nso = nso_from_elf(&elf, &BUILD_ID).unwrap();
        assert_eq!(&nso[..4], b"NSO0");
        assert_eq!(u32_at(&nso, 0x14).unwrap(), 0, "text at 0");
        assert_eq!(u32_at(&nso, 0x24).unwrap(), 0x1000);
        assert_eq!(u32_at(&nso, 0x34).unwrap(), 0x2000);
        assert_eq!(u32_at(&nso, 0x3C).unwrap(), 0x38, "the bss");
        assert_eq!(nso[0x100], 0xA0);
        assert_eq!(nso[u32_at(&nso, 0x30).unwrap() as usize], 0xA2);
        assert_eq!(&nso[0x40..0x60], &BUILD_ID);
        assert!(build_id_hex().starts_with("5e1f7e57"));
        elf[0x12] = 62;
        assert!(nso_from_elf(&elf, &BUILD_ID).is_err(), "x86-64");
    }

    #[test]
    fn the_change_changes_the_result() {
        assert_ne!(expected_result(5, 16400), expected_result(7, 16400));
        assert_ne!(expected_result(5, 16400), expected_result(5, 16401));
    }
}

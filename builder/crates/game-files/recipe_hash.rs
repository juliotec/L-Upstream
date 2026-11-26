// The romfs index's sources and their hash: read by build.rs (ROMFS_SOURCES, in romfs::RECIPE) and by romfs.rs's tests.

/// The files, under the crate's `src/`, that decide what an index holds: its entries (romfs.rs, the .nup's records and
/// how each is stored: nup.rs, zstd.rs, le.rs) and the damaged files' stand-ins (romfs.rs).
pub const FILES: &[&str] = &["romfs.rs", "nup.rs", "zstd.rs", "le.rs"];

/// FNV-1a, 64 bits, from `seed`.
fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The hash of `FILES` under `src` (each named by its file name, then its length and bytes), 128 bits as hex.
pub fn hash(src: &std::path::Path) -> String {
    let (mut a, mut b) = (0xcbf2_9ce4_8422_2325u64, 0x8422_2325_cbf2_9ce4u64);
    for name in FILES {
        let text = std::fs::read(src.join(name)).expect("a source file of the romfs index");
        for part in [name.as_bytes(), &(text.len() as u64).to_le_bytes(), &text] {
            a = fnv(a, part);
            b = fnv(b, part);
        }
    }
    format!("{a:016x}{b:016x}")
}

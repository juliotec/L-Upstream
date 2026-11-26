//! The checks on a linked image: the 2 GB limit its RIP-relative code can address (PE and ELF, each side of it, and a
//! file that is neither), and the renderer module's unresolved symbols with the C and C++ runtimes' versioned imports
//! left out.

use build_compile::link::{IMAGE_LIMIT, image_span, image_too_big};
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("link_guards").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A PE32+ header whose OptionalHeader.SizeOfImage is `size`.
fn pe(size: u32) -> Vec<u8> {
    let mut d = vec![0u8; 0x200];
    d[..2].copy_from_slice(b"MZ");
    d[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    d[0x80..0x84].copy_from_slice(b"PE\0\0");
    d[0x80 + 24..0x80 + 26].copy_from_slice(&0x20Bu16.to_le_bytes());
    d[0x80 + 24 + 56..0x80 + 24 + 60].copy_from_slice(&size.to_le_bytes());
    d
}

/// A 64-bit little-endian ELF with these PT_LOAD segments (vaddr, memsz) and a PT_NOTE that would reach further.
fn elf(loads: &[(u64, u64)]) -> Vec<u8> {
    let (phoff, phentsize) = (0x40usize, 0x38usize);
    let phnum = loads.len() + 1;
    let mut d = vec![0u8; phoff + phentsize * phnum];
    d[..4].copy_from_slice(b"\x7fELF");
    d[4] = 2;
    d[5] = 1;
    d[0x20..0x28].copy_from_slice(&(phoff as u64).to_le_bytes());
    d[0x36..0x38].copy_from_slice(&(phentsize as u16).to_le_bytes());
    d[0x38..0x3A].copy_from_slice(&(phnum as u16).to_le_bytes());
    let entries = loads.iter().map(|&(v, m)| (1u32, v, m)).chain([(4u32, 0, u64::MAX / 2)]);
    for (i, (kind, vaddr, memsz)) in entries.enumerate() {
        let e = phoff + i * phentsize;
        d[e..e + 4].copy_from_slice(&kind.to_le_bytes());
        d[e + 0x10..e + 0x18].copy_from_slice(&vaddr.to_le_bytes());
        d[e + 0x28..e + 0x30].copy_from_slice(&memsz.to_le_bytes());
    }
    d
}

#[test]
fn a_pe_image_each_side_of_the_limit() {
    let d = dir("pe");
    let limit = IMAGE_LIMIT as u32;
    for (size, big) in [(limit - 0x1000, false), (limit, true), (limit + 0x1000, true)] {
        let exe = d.join("game.exe");
        std::fs::write(&exe, pe(size)).unwrap();
        assert_eq!(image_span(&exe), Some(u64::from(size)));
        let said = image_too_big(&exe);
        assert_eq!(said.is_some(), big, "{size:#x}");
        assert_eq!(exe.exists(), !big, "an image past the limit is deleted, so nothing starts it");
        if let Some(s) = said {
            assert!(s.starts_with("link FAILED: the game's image is 2048 MB, past the 2048 MB"), "{s}");
        }
    }
}

#[test]
fn an_elf_image_spans_its_loads_only() {
    let d = dir("elf");
    let exe = d.join("game");
    let base = 0x40_0000u64;
    for (loads, span) in [
        (vec![(base, 0x1000), (base + 0x10_0000, IMAGE_LIMIT - 0x10_0000 - 1)], IMAGE_LIMIT - 1),
        (vec![(base + 0x10_0000, IMAGE_LIMIT - 0x10_0000), (base, 0x1000)], IMAGE_LIMIT),
    ] {
        std::fs::write(&exe, elf(&loads)).unwrap();
        assert_eq!(image_span(&exe), Some(span), "from the lowest load to the highest end, the note aside");
        assert_eq!(image_too_big(&exe).is_some(), span >= IMAGE_LIMIT);
    }
    std::fs::write(&exe, elf(&[(u64::MAX - 0x10, 0x1000)])).unwrap();
    assert_eq!(image_span(&exe), Some(0x10), "a segment's end past 2^64 does not wrap");
}

#[test]
fn a_file_that_is_neither_is_left_alone() {
    let d = dir("neither");
    let f = d.join("x");
    let mut not_pe = pe(u32::MAX);
    not_pe[0x80..0x84].copy_from_slice(b"NE\0\0");
    let mut elf32 = elf(&[(0, IMAGE_LIMIT * 2)]);
    elf32[4] = 1;
    for data in [b"#!/bin/sh\n".to_vec(), Vec::new(), b"MZ".to_vec(), not_pe, elf32] {
        std::fs::write(&f, &data).unwrap();
        assert_eq!(image_span(&f), None);
        assert_eq!(image_too_big(&f), None);
        assert!(f.exists());
    }
    assert_eq!(image_span(&d.join("missing")), None);
}

#[cfg(unix)]
#[test]
fn only_the_runtimes_versioned_imports_are_resolved() {
    use build_compile::link::vk_unresolved;
    use std::os::unix::fs::PermissionsExt;
    let d = dir("nm");
    // (what `nm -D --undefined-only` prints of the renderer's module: llvm-nm keeps the versions)
    let listing = "                 w __cxa_finalize@GLIBC_2.2.5\n\
                   \x20                U memcpy@GLIBC_2.14\n\
                   \x20                U __cxa_begin_catch@CXXABI_1.3\n\
                   \x20                U _ZNSt6vectorIiSaIiEE17_M_realloc_insertEv@GLIBCXX_3.4.21\n\
                   \x20                U _Unwind_Resume@GCC_3.0\n\
                   \x20                U ___tracy_emit_zone_begin\n\
                   \x20                U hos_frame_ready\n\
                   \x20                U _ZN12_GLOBAL__N_15g_cfgE\n";
    std::fs::write(d.join("listing.txt"), listing).unwrap();
    let nm = d.join("nm");
    std::fs::write(
        &nm,
        format!(
            "#!/bin/sh\n[ \"$1 $2\" = \"-D --undefined-only\" ] || exit 2\ncat '{}'\n",
            d.join("listing.txt").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&nm, std::fs::Permissions::from_mode(0o755)).unwrap();
    let module = d.join("liblongshot_vk.so");
    let bad = vk_unresolved(Some(&nm), &module, false).unwrap().unwrap();
    assert_eq!(bad, ["___tracy_emit_zone_begin", "hos_frame_ready", "_ZN12_GLOBAL__N_15g_cfgE"]);
    let bad = vk_unresolved(Some(&nm), &module, true).unwrap().unwrap();
    assert_eq!(bad, ["hos_frame_ready", "_ZN12_GLOBAL__N_15g_cfgE"], "a Tracy build has Tracy's C API");
    std::fs::write(d.join("listing.txt"), "                 U memcpy@GLIBC_2.14\n").unwrap();
    assert_eq!(vk_unresolved(Some(&nm), &module, false).unwrap(), None);
    assert_eq!(vk_unresolved(None, &module, false).unwrap(), None, "no nm: a guard, not a dependency");
    std::fs::write(&nm, "#!/bin/sh\nexit 1\n").unwrap();
    assert_eq!(vk_unresolved(Some(&nm), &module, false).unwrap(), None, "an nm that fails says nothing");
}

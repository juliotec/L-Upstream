//! GLOB_DAT and JUMP_SLOT with an addend write what Horizon's rtld writes, the symbol's address + A (AArch64 ELF ABI),
//! the symbol bound in another module (the image's first definition). The synthetic module's GOT and PLT words have
//! no addend, so the check before this one would pass with the addend dropped (ported from tests/test_relocations.py).

use super::tests::synthetic;
use super::*;

/// The synthetic module with its GLOB_DAT (0x350) and JUMP_SLOT (0x358) bound to `exp`, with these addends.
fn with_addends(glob_dat: u64, jump_slot: u64) -> Nso {
    let mut raw = synthetic();
    // (its RELA entries are 24 bytes from 0x180, in the segment at module offset 0x100, 0x100 into the file's body)
    let at = |k: usize, field: usize| 0x100 + 0x180 + 24 * k + field;
    for (k, kind, addend) in [(2, 1025u64, glob_dat), (3, 1026, jump_slot)] {
        raw[at(k, 8)..at(k, 16)].copy_from_slice(&((1u64 << 32) | kind).to_le_bytes());
        raw[at(k, 16)..at(k, 24)].copy_from_slice(&addend.to_le_bytes());
    }
    Nso::parse("main".into(), &raw).unwrap()
}

#[test]
fn glob_dat_and_jump_slot_add_their_addend() {
    let rtld = super::tests::module("rtld");
    let img = Image::build(vec![rtld, with_addends(0x18, 0x8)], &[], &Options::default()).unwrap();
    let word = |off: u64| img.u64_at(img.modules[1].base + off).unwrap();
    let exp = CODE_39BIT + 0x40;
    assert_eq!(word(0x350), exp + 0x18, "GLOB_DAT: S + A, S defined by another module");
    assert_eq!(word(0x358), exp + 0x8, "JUMP_SLOT: S + A");

    let img = Image::build(vec![super::tests::module("rtld"), with_addends(0, 0)], &[], &Options::default()).unwrap();
    assert_eq!(img.u64_at(img.modules[1].base + 0x358).unwrap(), exp, "JUMP_SLOT: S + 0");
}

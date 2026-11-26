//! What the walk and the jump-table recovery need of an instruction, from the decoder: its kind ([`a64::Kind`]),
//! whether it writes a register ([`a64::Insn::writes`]), and the addresses ADR, ADRP and ADD make.

pub use a64::Kind;

/// `w` may write X`reg` (31: SP).
pub(crate) fn writes(w: u32, reg: u32) -> bool {
    a64::decode(w).is_some_and(|i| u8::try_from(reg).is_ok_and(|r| i.writes(r)))
}

/// ADRP's page plus its offset, when `w` is an ADRP.
pub(crate) fn adrp(w: u32, pc: u64) -> Option<u64> {
    match a64::decode(w)? {
        i @ a64::Insn::Adr { page: true, .. } => i.adr_target(pc),
        _ => None,
    }
}

/// ADR's address, when `w` is an ADR.
pub(crate) fn adr(w: u32, pc: u64) -> Option<u64> {
    match a64::decode(w)? {
        i @ a64::Insn::Adr { page: false, .. } => i.adr_target(pc),
        _ => None,
    }
}

/// ADD Xd, Xn, #imm{, lsl 12}'s immediate, when `w` is one (64-bit, flags not set).
pub(crate) fn add_imm(w: u32) -> Option<u64> {
    match a64::decode(w)? {
        a64::Insn::AddSubImm { sf: true, sub: false, setflags: false, imm12, shift12, .. } => {
            Some(u64::from(imm12) << if shift12 { 12 } else { 0 })
        }
        _ => None,
    }
}

/// A load or store pair's byte offset from its base, when `w` is one with an offset.
pub(crate) fn pair_offset(w: u32) -> Option<i64> {
    match a64::decode(w)? {
        a64::Insn::Pair { addr: a64::Addr::Offset { imm, .. }, .. } => Some(imm),
        _ => None,
    }
}

/// A BL's target, when `w` at `pc` is one.
pub(crate) fn bl_target(w: u32, pc: u64) -> Option<u64> {
    match Kind::of(w, pc) {
        Kind::Bl(t) => Some(t),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adr_forms() {
        assert_eq!(adrp(0x9000_0000, 0x1234), Some(0x1000)); // adrp x0, #0
        assert_eq!(adrp(0xb000_0000, 0x1234), Some(0x2000)); // adrp x0, #1 page
        assert_eq!(adr(0x1000_0060, 0x1000), Some(0x100c)); // adr x0, #12
        assert_eq!(adr(0x9000_0000, 0x1234), None, "an adrp is no adr");
        assert_eq!(add_imm(0x9140_0400), Some(0x1000)); // add x0, x0, #1, lsl #12
        assert_eq!(add_imm(0xb100_0400), None, "adds sets flags");
        assert_eq!(pair_offset(0xa940_87e0), Some(8)); // ldp x0, x1, [sp, #8]
        assert_eq!(bl_target(0x9400_0002, 0x1000), Some(0x1008));
    }
}

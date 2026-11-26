//! An instruction's kind for the code that walks or places in the game's code (discovery, the sites): the branch it is
//! with its absolute target, or a plain operation. The decoder's view ([`Insn::flow`]), so a PAuth branch is the
//! branch it is, ERET and DRPS trap as they do at EL0, and a word that does not decode traps.

use crate::{Barrier, BranchRegOp, Flow, Insn, decode};

/// An instruction's kind. Targets are absolute addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// B: to the target.
    B(u64),
    /// BL: a call of the target.
    Bl(u64),
    /// B.cond: to the target or on.
    BCond(u64),
    /// CBZ, CBNZ: to the target or on.
    Cbz(u64),
    /// TBZ, TBNZ: to the target or on.
    Tbz(u64),
    /// BR (or BRAA ...) of the register.
    Br(u8),
    /// BLR (or BLRAA ...) of the register.
    Blr(u8),
    /// RET (or RETAA ...).
    Ret,
    /// SVC.
    Svc,
    /// BRK, HLT, UDF, HVC, SMC, DCPS, ERET, DRPS, or a word that does not decode.
    Trap,
    /// ISB.
    Isb,
    /// IC IVAU.
    Ic,
    /// Anything else.
    Op,
}

impl Kind {
    /// The kind of the word `w` at `pc`.
    pub fn of(w: u32, pc: u64) -> Kind {
        let to = |offset: i64| pc.wrapping_add_signed(offset);
        let Some(insn) = decode(w) else { return Kind::Trap };
        match insn {
            Insn::B { link: true, offset } => Kind::Bl(to(offset)),
            Insn::B { link: false, offset } => Kind::B(to(offset)),
            Insn::BCond { offset, .. } => Kind::BCond(to(offset)),
            Insn::Cbz { offset, .. } => Kind::Cbz(to(offset)),
            Insn::Tbz { offset, .. } => Kind::Tbz(to(offset)),
            Insn::BranchReg { op: BranchRegOp::Br, rn, .. } => Kind::Br(rn.0),
            Insn::BranchReg { op: BranchRegOp::Blr, rn, .. } => Kind::Blr(rn.0),
            Insn::Barrier { op: Barrier::Isb, .. } => Kind::Isb,
            Insn::Sys { l: false, op1: 3, crn: 7, crm: 5, op2: 1, .. } => Kind::Ic,
            i => match i.flow() {
                Flow::Return => Kind::Ret,
                Flow::Svc => Kind::Svc,
                Flow::Trap => Kind::Trap,
                _ => Kind::Op,
            },
        }
    }

    /// Its name (messages).
    pub fn name(self) -> &'static str {
        match self {
            Kind::B(_) => "b",
            Kind::Bl(_) => "bl",
            Kind::BCond(_) => "bcond",
            Kind::Cbz(_) => "cbz",
            Kind::Tbz(_) => "tbz",
            Kind::Br(_) => "br",
            Kind::Blr(_) => "blr",
            Kind::Ret => "ret",
            Kind::Svc => "svc",
            Kind::Trap => "trap",
            Kind::Isb => "isb",
            Kind::Ic => "ic",
            Kind::Op => "op",
        }
    }

    /// Whether it is a conditional branch (B.cond, CBZ, TBZ and their opposites).
    pub fn is_conditional(self) -> bool {
        matches!(self, Kind::BCond(_) | Kind::Cbz(_) | Kind::Tbz(_))
    }

    /// The branch target inside the function a walk follows: B's, and the conditional branches'.
    pub fn target(self) -> Option<u64> {
        match self {
            Kind::B(t) | Kind::BCond(t) | Kind::Cbz(t) | Kind::Tbz(t) => Some(t),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Kind;

    #[test]
    fn kinds() {
        let pc = 0x8000_1000;
        assert_eq!(Kind::of(0x1400_0002, pc), Kind::B(pc + 8));
        assert_eq!(Kind::of(0x97ff_ffff, pc), Kind::Bl(pc - 4));
        assert_eq!(Kind::of(0x54ff_ffc1, pc), Kind::BCond(pc - 8));
        assert_eq!(Kind::of(0x3400_0083, pc), Kind::Cbz(pc + 16));
        assert_eq!(Kind::of(0xb747_ff05, pc), Kind::Tbz(pc - 32));
        assert_eq!(Kind::of(0xd61f_0200, pc), Kind::Br(16));
        assert_eq!(Kind::of(0xd61f_083f, pc), Kind::Br(1), "braaz x1");
        assert_eq!(Kind::of(0xd73f_0822, pc), Kind::Blr(1), "blraa x1, x2");
        assert_eq!(Kind::of(0xd71f_0a11, pc), Kind::Br(16), "braa x16, x17");
        assert_eq!(Kind::of(0xd63f_0100, pc), Kind::Blr(8));
        assert_eq!(Kind::of(0xd65f_03c0, pc), Kind::Ret);
        assert_eq!(Kind::of(0xd65f_0bff, pc), Kind::Ret, "retaa");
        assert_eq!(Kind::of(0xd69f_03e0, pc), Kind::Trap, "eret traps at EL0");
        assert_eq!(Kind::of(0xd420_0020, pc), Kind::Trap);
        assert_eq!(Kind::of(0x0000_0000, pc), Kind::Trap, "udf");
        assert_eq!(Kind::of(0xd400_1001, pc), Kind::Svc);
        assert_eq!(Kind::of(0xd503_3fdf, pc), Kind::Isb);
        assert_eq!(Kind::of(0x9100_0400, pc), Kind::Op);
    }

    #[test]
    fn writes() {
        let w = |word: u32, x: u8| crate::decode(word).expect("an instruction").writes(x);
        assert!(w(0x8b02_0020, 0), "add x0, x1, x2");
        assert!(!w(0xf900_0020, 0), "str x0, [x1]");
        assert!(w(0xa940_07e0, 1), "ldp x0, x1, [sp]: Rt2");
        assert!(!w(0xa900_07e0, 1), "stp x0, x1, [sp]");
        assert!(!w(0xfd40_0020, 0), "ldr d0, [x1]: a V register");
        assert!(w(0xf81f_0fe0, 31), "str x0, [sp, #-16]!: SP written back");
        assert!(!w(0xf81f_0fe0, 0), "the stored register is read");
        assert!(w(0x9400_0001, 30) && !w(0x9400_0001, 0), "bl writes x30");
        assert!(!w(0xeb01_001f, 31), "cmp x0, x1: XZR is no write");
        assert!(w(0x9100_03ff, 31), "mov sp, sp (add): SP");
        assert!(w(0x9e66_0000, 0), "fmov x0, d0");
        assert!(w(0x1e20_0000, 0), "fcvtns w0, s0 writes the general register");
    }
}

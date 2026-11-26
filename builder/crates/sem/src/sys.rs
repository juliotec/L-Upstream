//! Hints, barriers, PSTATE and the system registers and operations EL0 has.

use crate::{Fault, Sem};
use a64::{Barrier, Insn, SysReg};
use mir::{BinOp, Flag, Loc, Op, Type, UnOp};

/// CNTFRQ_EL0: the Switch's 19.2 MHz (the runtime's counter, recomp_cntpct, counts at it).
const COUNTER_FREQUENCY: u64 = 19_200_000;
/// CTR_EL0: the Switch 2's cache type.
const CACHE_TYPE: u64 = 0x8444_c004;
/// CNTPCT_EL0, read as the virtual counter is.
const CNTPCT_EL0: SysReg = SysReg::new(3, 3, 14, 0, 1);
/// DCZID_EL0: DC ZVA allowed (DZP clear), its block 2^BS words, BS 4: the 64 bytes DC ZVA zeroes here.
const DCZID_EL0: SysReg = SysReg::new(3, 3, 0, 0, 7);
const ZVA_BLOCK: u64 = 4;

impl Sem<'_> {
    pub(crate) fn system(&mut self, insn: &Insn) -> Result<(), Fault> {
        match *insn {
            // (every hint is a no-op: NOP, YIELD, the PAuth hints (authentication is the identity), BTI)
            Insn::Hint { .. } => {}
            Insn::Clrex { .. } => {
                self.op(Op::Clrex);
            }
            Insn::Barrier { op: Barrier::Dsb | Barrier::Dmb | Barrier::Isb, .. } => {
                self.op(Op::Fence);
            }
            Insn::Pstate { op1: 0, op2: 0, .. } => {
                // CFINV
                let c = self.flag(Flag::C);
                let inverted = self.un(UnOp::Not, c);
                self.set(Loc::Flag(Flag::C), inverted);
            }
            Insn::Pstate { op1: 3, op2: 1, .. } => {} // (SSBS: speculation, no effect here)
            Insn::Pstate { .. } => return Err(Fault::Undefined), // (DAIF, SPSel, PAN, UAO: not at EL0)
            Insn::Sys { l: false, op1: 3, crn: 7, crm, op2: 1, rt } => match crm {
                4 => {
                    // DC ZVA: 64 zero bytes at the 64-byte block holding the address
                    let x = self.x(rt);
                    let at = self.and_k(x, !63u128 & u128::from(u64::MAX));
                    let zero = self.k64(0);
                    for k in 0..8u64 {
                        let off = self.k64(8 * k);
                        let a = self.bin(BinOp::Add, at, off);
                        self.op(Op::Store { addr: a, value: zero, space: mir::Space::Guest, order: None });
                    }
                    self.op(Op::Mark { addr: at, bytes: 64 });
                }
                // (DC CVAC, CVAU, CVAP, CIVAC and IC IVAU: no cache to maintain, no code that changes)
                5 | 10 | 11 | 12 | 13 | 14 => {}
                _ => return Err(Fault::Undefined),
            },
            Insn::Sys { .. } => return Err(Fault::Undefined),
            Insn::Mrs { reg, rt } => {
                let v = match reg {
                    SysReg::NZCV => self.nzcv(),
                    SysReg::FPCR => self.get(Loc::Fpcr),
                    SysReg::FPSR => self.get(Loc::Fpsr),
                    SysReg::TPIDR_EL0 => self.get(Loc::Tpidr),
                    SysReg::TPIDRRO_EL0 => self.get(Loc::TpidrRo),
                    SysReg::CNTVCT_EL0 | CNTPCT_EL0 => self.op(Op::Counter),
                    SysReg::CNTFRQ_EL0 => self.k64(COUNTER_FREQUENCY),
                    r if r == SysReg::new(3, 3, 0, 0, 1) => self.k64(CACHE_TYPE),
                    DCZID_EL0 => self.k64(ZVA_BLOCK),
                    _ => return Err(Fault::Undefined),
                };
                self.write(rt, v, false);
            }
            Insn::Msr { reg, rt } => {
                let x = self.x(rt);
                match reg {
                    SysReg::NZCV => {
                        let (n, z, c, v) = (self.bit(x, 31), self.bit(x, 30), self.bit(x, 29), self.bit(x, 28));
                        self.set_nzcv(n, z, c, v);
                    }
                    // (its upper half is RES0)
                    SysReg::FPCR => {
                        let v = self.and_k(x, 0xffff_ffff);
                        self.set(Loc::Fpcr, v);
                    }
                    SysReg::FPSR => self.set(Loc::Fpsr, x),
                    SysReg::TPIDR_EL0 => self.set(Loc::Tpidr, x),
                    _ => return Err(Fault::Undefined),
                }
            }
            _ => unreachable!("not a system instruction: {insn}"),
        }
        Ok(())
    }

    /// NZCV as MRS reads it: N at bit 31 down to V at bit 28.
    fn nzcv(&mut self) -> mir::Value {
        let mut acc = self.k64(0);
        for (flag, bit) in [(Flag::N, 31), (Flag::Z, 30), (Flag::C, 29), (Flag::V, 28)] {
            let f = self.flag(flag);
            let wide = self.zext(f, Type::I64);
            let at = self.shl_k(wide, bit);
            acc = self.bin(BinOp::Or, acc, at);
        }
        acc
    }
}

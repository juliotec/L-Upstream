//! An instruction as text, the way LLVM's AArch64 disassembler writes it with `-M no-aliases` (which still applies the
//! aliases its printer writes by hand: the bitfield moves, MOVZ/MOVN and some ORRs as MOV, BFC). Its use is checking the
//! decoder against llvm-mc, and naming instructions in the lifter's messages.

use crate::*;
use std::fmt::{self, Display, Formatter};

mod simd;

/// A general register as an operand: 31 is SP or the zero register, as the operand's encoding says.
#[derive(Clone, Copy)]
struct Gpr {
    reg: Reg,
    x64: bool,
    sp: bool,
}

impl Display for Gpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match (self.reg.0, self.x64, self.sp) {
            (31, true, true) => f.write_str("sp"),
            (31, false, true) => f.write_str("wsp"),
            (31, true, false) => f.write_str("xzr"),
            (31, false, false) => f.write_str("wzr"),
            (n, true, _) => write!(f, "x{n}"),
            (n, false, _) => write!(f, "w{n}"),
        }
    }
}

fn r(x64: bool, reg: Reg) -> Gpr {
    Gpr { reg, x64, sp: false }
}
fn rsp(x64: bool, reg: Reg) -> Gpr {
    Gpr { reg, x64, sp: true }
}
fn x(reg: Reg) -> Gpr {
    r(true, reg)
}
fn w(reg: Reg) -> Gpr {
    r(false, reg)
}
fn xsp(reg: Reg) -> Gpr {
    rsp(true, reg)
}

/// A SIMD and FP register as a scalar of the access's size: b0, h0, s0, d0, q0.
struct Fpr(Size, Reg);

impl Display for Fpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", ["b", "h", "s", "d", "q"][usize::from(self.0.0)], self.1.0)
    }
}

/// `, lsl #3`, or nothing for LSL #0.
struct Shifted(Shift, u8);

impl Display for Shifted {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Shifted(Shift::Lsl, 0) => Ok(()),
            Shifted(s, n) => write!(f, ", {} #{n}", s.name()),
        }
    }
}

/// An immediate in hex the way LLVM writes it: 0 as `0`.
struct Hex(u16);

impl Display for Hex {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.0 == 0 { f.write_str("0") } else { write!(f, "{:#x}", self.0) }
    }
}

/// A memory operand of an access of `size`.
struct Mem(Addr, Size);

impl Display for Mem {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Mem(addr, size) = *self;
        match addr {
            Addr::Offset { rn, imm } => write!(f, "[{}, #{imm}]", xsp(rn)),
            Addr::PreIndex { rn, imm } => write!(f, "[{}, #{imm}]!", xsp(rn)),
            Addr::PostIndex { rn, imm } => write!(f, "[{}], #{imm}", xsp(rn)),
            Addr::RegOffset { rn, rm, extend, shifted } => {
                let index = if extend.is_64() { x(rm) } else { w(rm) };
                // (LLVM's printMemExtend: LSL always shows the access size's shift, whatever the S bit)
                if extend == Extend::Uxtx {
                    write!(f, "[{}, {index}, lsl #{}]", xsp(rn), size.0)
                } else if shifted {
                    write!(f, "[{}, {index}, {} #{}]", xsp(rn), extend.name(), size.0)
                } else {
                    write!(f, "[{}, {index}, {}]", xsp(rn), extend.name())
                }
            }
            Addr::Literal { imm } => write!(f, "#{imm}"),
        }
    }
}

/// A PRFM's operation: its name, or `#n` for the reserved ones.
struct Prefetch(u8);

impl Display for Prefetch {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let op = self.0;
        match (op >> 3, (op >> 1) & 3) {
            _ if op == 24 => f.write_str("ir"), // (PRFM IR, v8.9: a no-op prefetch on the A78C)
            (3, _) => write!(f, "#{op}"),
            (kind, target) => write!(
                f,
                "{}{}{}",
                ["pld", "pli", "pst"][usize::from(kind)],
                ["l1", "l2", "l3", "slc"][usize::from(target)],
                if op & 1 == 0 { "keep" } else { "strm" }
            ),
        }
    }
}

impl Shift {
    /// lsl, lsr, asr, ror.
    pub fn name(self) -> &'static str {
        match self {
            Shift::Lsl => "lsl",
            Shift::Lsr => "lsr",
            Shift::Asr => "asr",
            Shift::Ror => "ror",
        }
    }
}

impl Extend {
    /// uxtb ... sxtx.
    pub fn name(self) -> &'static str {
        match self {
            Extend::Uxtb => "uxtb",
            Extend::Uxth => "uxth",
            Extend::Uxtw => "uxtw",
            Extend::Uxtx => "uxtx",
            Extend::Sxtb => "sxtb",
            Extend::Sxth => "sxth",
            Extend::Sxtw => "sxtw",
            Extend::Sxtx => "sxtx",
        }
    }
}

impl Cond {
    /// eq, ne, hs ... (LLVM's names: hs and lo, not cs and cc).
    pub fn name(self) -> &'static str {
        ["eq", "ne", "hs", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al", "nv"]
            [usize::from(self.0 & 15)]
    }
}

impl Size {
    /// The mnemonic suffix of a byte or halfword access: b, h, or nothing.
    fn suffix(self) -> &'static str {
        match self.0 {
            0 => "b",
            1 => "h",
            _ => "",
        }
    }
}

fn barrier_option(crm: u8) -> Option<&'static str> {
    Some(match crm {
        15 => "sy",
        14 => "st",
        13 => "ld",
        11 => "ish",
        10 => "ishst",
        9 => "ishld",
        7 => "nsh",
        6 => "nshst",
        5 => "nshld",
        3 => "osh",
        2 => "oshst",
        1 => "oshld",
        _ => return None,
    })
}

fn acq_rel(acquire: bool, release: bool) -> &'static str {
    match (acquire, release) {
        (false, false) => "",
        (true, false) => "a",
        (false, true) => "l",
        (true, true) => "al",
    }
}

/// The system registers the game is known to use, with LLVM's names: (register, name, read-only).
const SYSREG_NAMES: [(SysReg, &str, bool); 11] = [
    (SysReg::NZCV, "NZCV", false),
    (SysReg::FPCR, "FPCR", false),
    (SysReg::FPSR, "FPSR", false),
    (SysReg::TPIDR_EL0, "TPIDR_EL0", false),
    (SysReg::TPIDRRO_EL0, "TPIDRRO_EL0", false),
    (SysReg::CNTVCT_EL0, "CNTVCT_EL0", true),
    (SysReg::CNTFRQ_EL0, "CNTFRQ_EL0", false),
    (SysReg::new(3, 3, 0, 0, 1), "CTR_EL0", true),
    (SysReg::new(3, 3, 0, 0, 7), "DCZID_EL0", true),
    (SysReg::new(3, 0, 0, 0, 0), "MIDR_EL1", true),
    (SysReg::new(3, 3, 4, 2, 6), "SSBS", false),
];

/// A system register's name as MRS (`write` false) or MSR writes it: its name if this crate knows it (and, for MSR, it
/// can be written), else the generic `S3_3_C4_C2_0` form.
pub fn sysreg_name(reg: SysReg, write: bool) -> String {
    SYSREG_NAMES
        .iter()
        .find(|(r, _, read_only)| *r == reg && !(write && *read_only))
        .map_or_else(|| generic_sysreg(reg), |(_, name, _)| (*name).to_string())
}

/// The generic name of a system register: `S<op0>_<op1>_C<n>_C<m>_<op2>`.
pub fn generic_sysreg(reg: SysReg) -> String {
    let (op0, op1, crn, crm, op2) = reg.fields();
    format!("S{op0}_{op1}_C{crn}_C{crm}_{op2}")
}

/// LLVM's isAnyMOVZMovAlias: one MOVZ makes `value`.
fn any_movz(value: u64, width: u32) -> bool {
    (0..=width - 16).step_by(16).any(|sh| value & !(0xffffu64 << sh) == 0)
}

/// LLVM's isAnyMOVWMovAlias: one MOVZ or one MOVN makes `value`.
fn any_movw(value: u64, width: u32) -> bool {
    let inverted = if width == 32 { !value & 0xffff_ffff } else { !value };
    any_movz(value, width) || any_movz(inverted, width)
}

/// LLVM's isMOVNMovAlias: MOVN prints as MOV unless MOVZ could make the value.
fn movn_is_mov(value: u64, shift: u32, width: u32) -> bool {
    if any_movz(value, width) {
        return false;
    }
    let inverted = if width == 32 { !value & 0xffff_ffff } else { !value };
    !(inverted == 0 && shift != 0) && inverted & !(0xffffu64 << shift) == 0
}

/// A register's value as a signed number of its width, as LLVM writes MOV's immediate.
fn signed(sf: bool, v: u64) -> i64 {
    if sf { v as i64 } else { i64::from(v as u32 as i32) }
}

fn add_sub(sub: bool, setflags: bool) -> &'static str {
    match (sub, setflags) {
        (false, false) => "add",
        (false, true) => "adds",
        (true, false) => "sub",
        (true, true) => "subs",
    }
}

impl Display for Insn {
    /// llvm-mc's no-alias disassembly: the mnemonic, a space, the operands.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match *self {
            Insn::Adr { page, rd, imm } => write!(f, "{} {}, #{imm}", if page { "adrp" } else { "adr" }, x(rd)),
            Insn::AddSubImm { sf, sub, setflags, rd, rn, imm12, shift12 } => {
                let d = if setflags { r(sf, rd) } else { rsp(sf, rd) };
                write!(f, "{} {d}, {}, #{imm12}", add_sub(sub, setflags), rsp(sf, rn))?;
                if shift12 { f.write_str(", lsl #12") } else { Ok(()) }
            }
            Insn::LogicalImm { sf, op, rd, rn, imm } => {
                let d = if op == LogicOp::Ands { r(sf, rd) } else { rsp(sf, rd) };
                if op == LogicOp::Orr && rn.is_31() && !any_movw(imm, if sf { 64 } else { 32 }) {
                    // (LLVM's printer: ORR from the zero register of a value no MOVZ or MOVN makes is MOV)
                    write!(f, "mov {d}, #{}", signed(sf, imm))
                } else {
                    write!(f, "{} {d}, {}, #{imm:#x}", op.name(), r(sf, rn))
                }
            }
            Insn::MoveWide { sf, op, rd, imm16, hw } => {
                let shift = u32::from(hw) * 16;
                let mask = if sf { u64::MAX } else { 0xffff_ffff };
                let value = u64::from(imm16) << shift;
                match op {
                    MoveOp::Movz if !(value == 0 && shift != 0) => {
                        write!(f, "mov {}, #{}", r(sf, rd), signed(sf, value))
                    }
                    MoveOp::Movn if movn_is_mov(!value & mask, shift, if sf { 64 } else { 32 }) => {
                        write!(f, "mov {}, #{}", r(sf, rd), signed(sf, !value & mask))
                    }
                    _ => {
                        let m = match op {
                            MoveOp::Movn => "movn",
                            MoveOp::Movz => "movz",
                            MoveOp::Movk => "movk",
                        };
                        write!(f, "{m} {}, #{imm16}", r(sf, rd))?;
                        if shift != 0 { write!(f, ", lsl #{shift}") } else { Ok(()) }
                    }
                }
            }
            Insn::Bitfield { sf, op, rd, rn, immr, imms } => bitfield(f, sf, op, rd, rn, immr, imms),
            Insn::Extr { sf, rd, rn, rm, lsb } => write!(f, "extr {}, {}, {}, #{lsb}", r(sf, rd), r(sf, rn), r(sf, rm)),
            Insn::B { link, offset } => write!(f, "{} #{offset}", if link { "bl" } else { "b" }),
            Insn::BCond { cond, offset } => write!(f, "b.{} #{offset}", cond.name()),
            Insn::Cbz { sf, nonzero, rt, offset } => {
                write!(f, "{} {}, #{offset}", if nonzero { "cbnz" } else { "cbz" }, r(sf, rt))
            }
            Insn::Tbz { nonzero, rt, bit, offset } => {
                write!(f, "{} {}, #{bit}, #{offset}", if nonzero { "tbnz" } else { "tbz" }, r(bit >= 32, rt))
            }
            Insn::BranchReg { op, rn, auth } => branch_reg(f, op, rn, auth),
            Insn::Svc { imm } => write!(f, "svc #{}", Hex(imm)),
            Insn::Hvc { imm } => write!(f, "hvc #{}", Hex(imm)),
            Insn::Smc { imm } => write!(f, "smc #{}", Hex(imm)),
            Insn::Brk { imm } => write!(f, "brk #{}", Hex(imm)),
            Insn::Hlt { imm } => write!(f, "hlt #{}", Hex(imm)),
            Insn::Dcps { level, imm: 0 } => write!(f, "dcps{level}"),
            Insn::Dcps { level, imm } => write!(f, "dcps{level} #{}", Hex(imm)),
            Insn::Udf { imm } => write!(f, "udf #{imm}"),
            Insn::Hint { imm } => write!(f, "hint #{imm}"),
            Insn::Clrex { crm } => write!(f, "clrex #{crm}"),
            Insn::Barrier { op: Barrier::Isb, crm: 15 } => f.write_str("isb sy"),
            Insn::Barrier { op: Barrier::Isb, crm } => write!(f, "isb #{crm}"),
            Insn::Barrier { op, crm } => {
                write!(f, "{} ", if op == Barrier::Dsb { "dsb" } else { "dmb" })?;
                match barrier_option(crm) {
                    Some(name) => f.write_str(name),
                    None => write!(f, "#{crm}"),
                }
            }
            Insn::Pstate { op1: 0, op2: 0, .. } => f.write_str("cfinv"),
            Insn::Pstate { op1, op2, crm } => {
                let name = match (op1, op2) {
                    (0, 3) => "UAO",
                    (0, 4) => "PAN",
                    (0, 5) => "SPSel",
                    (3, 1) => "SSBS",
                    (3, 6) => "DAIFSet",
                    _ => "DAIFClr",
                };
                write!(f, "msr {name}, #{crm}")
            }
            Insn::Sys { l: true, op1, crn, crm, op2, rt } => {
                write!(f, "sysl {}, #{op1}, c{crn}, c{crm}, #{op2}", x(rt))
            }
            Insn::Sys { l: false, op1, crn, crm, op2, rt } => {
                write!(f, "sys #{op1}, c{crn}, c{crm}, #{op2}, {}", x(rt))
            }
            Insn::Mrs { reg, rt } => write!(f, "mrs {}, {}", x(rt), sysreg_name(reg, false)),
            Insn::Msr { reg, rt } => write!(f, "msr {}, {}", sysreg_name(reg, true), x(rt)),
            Insn::LoadStore { kind, flavour, size, rt, addr } => load_store(f, kind, flavour, size, rt, addr),
            Insn::Pair { kind, nontemporal, size, rt, rt2, addr } => {
                let store = matches!(kind, PairKind::Store | PairKind::StoreFp);
                let nt = if nontemporal { "n" } else { "" };
                let sw = if kind == PairKind::LoadSignedWord { "sw" } else { "" };
                write!(f, "{}{nt}p{sw} ", if store { "st" } else { "ld" })?;
                match kind {
                    PairKind::StoreFp | PairKind::LoadFp => write!(f, "{}, {}", Fpr(size, rt), Fpr(size, rt2))?,
                    PairKind::LoadSignedWord => write!(f, "{}, {}", x(rt), x(rt2))?,
                    PairKind::Store | PairKind::Load => write!(f, "{}, {}", r(size.0 == 3, rt), r(size.0 == 3, rt2))?,
                }
                write!(f, ", {}", Mem(addr, size))
            }
            Insn::Exclusive { load, ordered, size, rs, rt, rt2, rn } => {
                let x64 = size.0 == 3;
                let order = match (ordered, load) {
                    (false, _) => "",
                    (true, true) => "a",
                    (true, false) => "l",
                };
                let form = if rt2.is_some() { "p" } else { "r" };
                let suffix = if rt2.is_some() { "" } else { size.suffix() };
                write!(f, "{}{order}x{form}{suffix} ", if load { "ld" } else { "st" })?;
                if !load {
                    write!(f, "{}, ", w(rs))?;
                }
                write!(f, "{}, ", r(x64, rt))?;
                if let Some(rt2) = rt2 {
                    write!(f, "{}, ", r(x64, rt2))?;
                }
                write!(f, "[{}]", xsp(rn))
            }
            Insn::Ordered { load, limited, size, rt, rn } => {
                let m = match (load, limited) {
                    (true, false) => "ldar",
                    (true, true) => "ldlar",
                    (false, false) => "stlr",
                    (false, true) => "stllr",
                };
                write!(f, "{m}{} {}, [{}]", size.suffix(), r(size.0 == 3, rt), xsp(rn))
            }
            Insn::Ldapr { size, rt, rn } => write!(f, "ldapr{} {}, [{}]", size.suffix(), r(size.0 == 3, rt), xsp(rn)),
            Insn::Cas { pair: true, size, acquire, release, rs, rt, rn } => {
                let x64 = size.0 == 3;
                let next = |q: Reg| r(x64, Reg(q.0 + 1));
                write!(
                    f,
                    "casp{} {}, {}, {}, {}, [{}]",
                    acq_rel(acquire, release),
                    r(x64, rs),
                    next(rs),
                    r(x64, rt),
                    next(rt),
                    xsp(rn)
                )
            }
            Insn::Cas { pair: false, size, acquire, release, rs, rt, rn } => {
                let x64 = size.0 == 3;
                let order = acq_rel(acquire, release);
                write!(f, "cas{order}{} {}, {}, [{}]", size.suffix(), r(x64, rs), r(x64, rt), xsp(rn))
            }
            Insn::Atomic { op, size, acquire, release, rs, rt, rn } => {
                let x64 = size.0 == 3;
                let order = acq_rel(acquire, release);
                write!(f, "{}{order}{} {}, {}, [{}]", op.name(), size.suffix(), r(x64, rs), r(x64, rt), xsp(rn))
            }
            Insn::LdrPac { b, writeback, rt, rn, imm } => {
                write!(f, "{} {}, [{}, #{imm}]", if b { "ldrab" } else { "ldraa" }, x(rt), xsp(rn))?;
                if writeback { f.write_str("!") } else { Ok(()) }
            }
            Insn::LogicalReg { sf, op, rd, rn, rm, shift, amount } => {
                write!(f, "{} {}, {}, {}{}", op.name(), r(sf, rd), r(sf, rn), r(sf, rm), Shifted(shift, amount))
            }
            Insn::AddSubReg { sf, sub, setflags, rd, rn, rm, shift, amount } => write!(
                f,
                "{} {}, {}, {}{}",
                add_sub(sub, setflags),
                r(sf, rd),
                r(sf, rn),
                r(sf, rm),
                Shifted(shift, amount)
            ),
            Insn::AddSubExt { sf, sub, setflags, rd, rn, rm, extend, amount } => {
                let d = if setflags { r(sf, rd) } else { rsp(sf, rd) };
                let index = if sf && extend.is_64() { x(rm) } else { w(rm) };
                write!(f, "{} {d}, {}, {index}", add_sub(sub, setflags), rsp(sf, rn))?;
                // (LLVM's printArithExtend: with SP on either side, the extend of the register's own width is LSL)
                let uses_sp = (!setflags && rd.is_31()) || rn.is_31();
                let natural = if sf { Extend::Uxtx } else { Extend::Uxtw };
                match (uses_sp && extend == natural, amount) {
                    (true, 0) => Ok(()),
                    (true, n) => write!(f, ", lsl #{n}"),
                    (false, 0) => write!(f, ", {}", extend.name()),
                    (false, n) => write!(f, ", {} #{n}", extend.name()),
                }
            }
            Insn::AddSubCarry { sf, sub, setflags, rd, rn, rm } => {
                let m = match (sub, setflags) {
                    (false, false) => "adc",
                    (false, true) => "adcs",
                    (true, false) => "sbc",
                    (true, true) => "sbcs",
                };
                write!(f, "{m} {}, {}, {}", r(sf, rd), r(sf, rn), r(sf, rm))
            }
            Insn::Rmif { rn, shift, mask } => write!(f, "rmif {}, #{shift}, #{mask}", x(rn)),
            Insn::Setf { sixteen, rn } => write!(f, "setf{} {}", if sixteen { 16 } else { 8 }, w(rn)),
            Insn::CondCompare { sf, negate, rn, rhs, nzcv, cond } => {
                write!(f, "{} {}, ", if negate { "ccmn" } else { "ccmp" }, r(sf, rn))?;
                match rhs {
                    CmpRhs::Reg(q) => write!(f, "{}", r(sf, q))?,
                    CmpRhs::Imm(i) => write!(f, "#{i}")?,
                }
                write!(f, ", #{nzcv}, {}", cond.name())
            }
            Insn::CondSelect { sf, op, rd, rn, rm, cond } => {
                write!(f, "{} {}, {}, {}, {}", op.name(), r(sf, rd), r(sf, rn), r(sf, rm), cond.name())
            }
            Insn::DataProc1 { sf, op, rd, rn } => write!(f, "{} {}, {}", op.name(), r(sf, rd), r(sf, rn)),
            Insn::DataProc2 { op: DataProc2Op::Pacga, rd, rn, rm, .. } => {
                write!(f, "pacga {}, {}, {}", x(rd), x(rn), xsp(rm))
            }
            Insn::DataProc2 { sf, op, rd, rn, rm } => {
                write!(f, "{} {}, {}, {}", op.name(), r(sf, rd), r(sf, rn), r(sf, rm))
            }
            Insn::Crc32 { castagnoli, size, rd, rn, rm } => write!(
                f,
                "crc32{}{} {}, {}, {}",
                if castagnoli { "c" } else { "" },
                ["b", "h", "w", "x"][usize::from(size.0)],
                w(rd),
                w(rn),
                r(size.0 == 3, rm)
            ),
            Insn::Pac { op, rd, rn } => {
                let (kind, key) = op.key.parts();
                let verb = if op.auth { "aut" } else { "pac" };
                match rn {
                    Some(rn) => write!(f, "{verb}{kind}{key} {}, {}", x(rd), xsp(rn)),
                    None => write!(f, "{verb}{kind}z{key} {}", x(rd)),
                }
            }
            Insn::Xpac { data, rd } => write!(f, "{} {}", if data { "xpacd" } else { "xpaci" }, x(rd)),
            Insn::DataProc3 { sf, op, rd, rn, rm, ra } => match op {
                DataProc3Op::Smulh | DataProc3Op::Umulh => write!(f, "{} {}, {}, {}", op.name(), x(rd), x(rn), x(rm)),
                DataProc3Op::Madd | DataProc3Op::Msub => {
                    write!(f, "{} {}, {}, {}, {}", op.name(), r(sf, rd), r(sf, rn), r(sf, rm), r(sf, ra))
                }
                _ => write!(f, "{} {}, {}, {}, {}", op.name(), x(rd), w(rn), w(rm), x(ra)),
            },
            Insn::Fp1 { .. }
            | Insn::FpCvt { .. }
            | Insn::Fp2 { .. }
            | Insn::Fp3 { .. }
            | Insn::FpCmp { .. }
            | Insn::FpCondCmp { .. }
            | Insn::FpCsel { .. }
            | Insn::FpImm { .. }
            | Insn::FpToInt { .. }
            | Insn::IntToFp { .. }
            | Insn::FmovGen { .. }
            | Insn::Simd3 { .. }
            | Insn::SimdDiff { .. }
            | Insn::Simd2 { .. }
            | Insn::SimdAcross { .. }
            | Insn::SimdPairwise { .. }
            | Insn::SimdShift { .. }
            | Insn::SimdElem { .. }
            | Insn::SimdPermute { .. }
            | Insn::SimdExt { .. }
            | Insn::SimdTable { .. }
            | Insn::SimdDupElem { .. }
            | Insn::SimdDupGen { .. }
            | Insn::SimdInsElem { .. }
            | Insn::SimdInsGen { .. }
            | Insn::SimdMovToGen { .. }
            | Insn::SimdModImm { .. }
            | Insn::Aes { .. }
            | Insn::Sha3 { .. }
            | Insn::Sha2 { .. }
            | Insn::SimdLdSt { .. } => simd::fmt(f, self),
        }
    }
}

impl LogicOp {
    /// and, bic ... bics.
    pub fn name(self) -> &'static str {
        match self {
            LogicOp::And => "and",
            LogicOp::Bic => "bic",
            LogicOp::Orr => "orr",
            LogicOp::Orn => "orn",
            LogicOp::Eor => "eor",
            LogicOp::Eon => "eon",
            LogicOp::Ands => "ands",
            LogicOp::Bics => "bics",
        }
    }
}

impl AtomicOp {
    /// ldadd ... swp (without the ordering and size suffixes).
    pub fn name(self) -> &'static str {
        match self {
            AtomicOp::Add => "ldadd",
            AtomicOp::Clr => "ldclr",
            AtomicOp::Eor => "ldeor",
            AtomicOp::Set => "ldset",
            AtomicOp::Smax => "ldsmax",
            AtomicOp::Smin => "ldsmin",
            AtomicOp::Umax => "ldumax",
            AtomicOp::Umin => "ldumin",
            AtomicOp::Swp => "swp",
        }
    }
}

impl CselOp {
    /// csel, csinc, csinv, csneg.
    pub fn name(self) -> &'static str {
        match self {
            CselOp::Csel => "csel",
            CselOp::Csinc => "csinc",
            CselOp::Csinv => "csinv",
            CselOp::Csneg => "csneg",
        }
    }
}

impl DataProc1Op {
    /// rbit ... cls.
    pub fn name(self) -> &'static str {
        match self {
            DataProc1Op::Rbit => "rbit",
            DataProc1Op::Rev16 => "rev16",
            DataProc1Op::Rev32 => "rev32",
            DataProc1Op::Rev => "rev",
            DataProc1Op::Clz => "clz",
            DataProc1Op::Cls => "cls",
        }
    }
}

impl DataProc2Op {
    /// udiv ... pacga (the shifts as LLVM names them: lsl, not lslv).
    pub fn name(self) -> &'static str {
        match self {
            DataProc2Op::Udiv => "udiv",
            DataProc2Op::Sdiv => "sdiv",
            DataProc2Op::Lslv => "lsl",
            DataProc2Op::Lsrv => "lsr",
            DataProc2Op::Asrv => "asr",
            DataProc2Op::Rorv => "ror",
            DataProc2Op::Pacga => "pacga",
        }
    }
}

impl DataProc3Op {
    /// madd ... umulh.
    pub fn name(self) -> &'static str {
        match self {
            DataProc3Op::Madd => "madd",
            DataProc3Op::Msub => "msub",
            DataProc3Op::Smaddl => "smaddl",
            DataProc3Op::Smsubl => "smsubl",
            DataProc3Op::Smulh => "smulh",
            DataProc3Op::Umaddl => "umaddl",
            DataProc3Op::Umsubl => "umsubl",
            DataProc3Op::Umulh => "umulh",
        }
    }
}

impl PacKey {
    /// ("i" or "d", "a" or "b"): what a mnemonic spells the key with (PACIA, AUTDB, PACIZA ...).
    fn parts(self) -> (&'static str, &'static str) {
        match self {
            PacKey::Ia => ("i", "a"),
            PacKey::Ib => ("i", "b"),
            PacKey::Da => ("d", "a"),
            PacKey::Db => ("d", "b"),
        }
    }
}

fn bitfield(f: &mut Formatter<'_>, sf: bool, op: BitfieldOp, rd: Reg, rn: Reg, immr: u8, imms: u8) -> fmt::Result {
    let width: u8 = if sf { 64 } else { 32 };
    let (d, n) = (r(sf, rd), r(sf, rn));
    if op == BitfieldOp::Bfm {
        return if rn.is_31() && (immr == 0 || imms < immr) {
            // (LLVM: BFC takes its whole range, which reaches further than BFI's)
            write!(f, "bfc {d}, #{}, #{}", (width - immr) % width, imms + 1)
        } else if imms < immr {
            write!(f, "bfi {d}, {n}, #{}, #{}", (width - immr) % width, imms + 1)
        } else {
            write!(f, "bfxil {d}, {n}, #{immr}, #{}", imms - immr + 1)
        };
    }
    let signed = op == BitfieldOp::Sbfm;
    let extend = match (immr, imms, signed, sf) {
        (0, 7, true, _) => Some("sxtb"),
        (0, 15, true, _) => Some("sxth"),
        (0, 31, true, true) => Some("sxtw"),
        (0, 7, false, false) => Some("uxtb"),
        (0, 15, false, false) => Some("uxth"),
        _ => None,
    };
    if let Some(name) = extend {
        write!(f, "{name} {d}, {}", w(rn))
    } else if imms == width - 1 {
        write!(f, "{} {d}, {n}, #{immr}", if signed { "asr" } else { "lsr" })
    } else if !signed && imms + 1 == immr {
        write!(f, "lsl {d}, {n}, #{}", width - 1 - imms)
    } else if imms < immr {
        write!(f, "{} {d}, {n}, #{}, #{}", if signed { "sbfiz" } else { "ubfiz" }, width - immr, imms + 1)
    } else {
        write!(f, "{} {d}, {n}, #{immr}, #{}", if signed { "sbfx" } else { "ubfx" }, imms - immr + 1)
    }
}

fn branch_reg(f: &mut Formatter<'_>, op: BranchRegOp, rn: Reg, auth: BranchAuth) -> fmt::Result {
    let base = match op {
        BranchRegOp::Br => "br",
        BranchRegOp::Blr => "blr",
        BranchRegOp::Ret => "ret",
        BranchRegOp::Eret => "eret",
        BranchRegOp::Drps => "drps",
    };
    let key = |b: bool| if b { "b" } else { "a" };
    match (op, auth) {
        (BranchRegOp::Eret | BranchRegOp::Drps, BranchAuth::None) => f.write_str(base),
        (_, BranchAuth::None) => write!(f, "{base} {}", x(rn)),
        (_, BranchAuth::Sp { b }) => write!(f, "{base}a{}", key(b)),
        (_, BranchAuth::Zero { b }) => write!(f, "{base}a{}z {}", key(b), x(rn)),
        (_, BranchAuth::Reg { b, rm }) => write!(f, "{base}a{} {}, {}", key(b), x(rn), xsp(rm)),
    }
}

fn load_store(f: &mut Formatter<'_>, kind: LsKind, flavour: LsFlavour, size: Size, rt: Reg, addr: Addr) -> fmt::Result {
    let store = matches!(kind, LsKind::Store | LsKind::StoreFp);
    let base = match (kind, flavour) {
        (LsKind::Prefetch, LsFlavour::Unscaled) => "prfum",
        (LsKind::Prefetch, _) => "prfm",
        (_, LsFlavour::Normal) if store => "str",
        (_, LsFlavour::Unscaled) if store => "stur",
        (_, LsFlavour::Unprivileged) if store => "sttr",
        (_, LsFlavour::Normal) => "ldr",
        (_, LsFlavour::Unscaled) => "ldur",
        (_, LsFlavour::Unprivileged) => "ldtr",
    };
    let mem = Mem(addr, size);
    match kind {
        LsKind::Store | LsKind::Load => write!(f, "{base}{} {}, {mem}", size.suffix(), r(size.0 == 3, rt)),
        LsKind::LoadSigned { x64 } => {
            write!(f, "{base}{} {}, {mem}", ["sb", "sh", "sw"][usize::from(size.0)], r(x64, rt))
        }
        LsKind::StoreFp | LsKind::LoadFp => write!(f, "{base} {}, {mem}", Fpr(size, rt)),
        LsKind::Prefetch => write!(f, "{base} {}, {mem}", Prefetch(rt.0)),
    }
}

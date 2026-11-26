//! `longshot-builder dev words --in <file> --pc <hex> --out <file.ll> --list <file> [--seq <file>] [--count-checks]`: each
//! instruction word
//! of `--in` (hex, one a line) that falls through and has semantics, as a function of its own (`f_<pc + 4 i>` for the
//! i-th word: the instruction at that address, then a return), for tests/test_lifter_semantics.py to run beside an
//! emulator. `--seq`: a line per sequence, `<pc> <word> <word> ...` (hex), each as one function more (`f_<pc>`: the
//! words at `pc`, `pc + 4`, ..., then a return), so that the check also runs them one after another, each the next
//! one's input; every word of a sequence must lift (the check takes them from the listing). `--count-checks`: the fast paths count their runs and their slow ones ([`llvm::Options::count_checks`]).
//!
//! `--list` gets a line per word written: `<index> <word> <kind> <base> <index register> <bytes> <text>`, where kind is
//! `alu`, `mem` (it reads or writes memory: the check points its base register at a page of its own, and its index
//! register at a small offset) or `lit` (a literal load, from near the instruction's own address), `-` where a field
//! has nothing, and text the instruction as LLVM writes it. Words that are left out are not listed: those that do not
//! fall through or have no semantics, and those whose result the architecture leaves CONSTRAINED UNPREDICTABLE (a
//! base written back that is also a register the access moves, a load pair into one register, an exclusive store
//! whose status register is one it reads), where an emulator and the hardware may each choose.

use a64::{Addr, Insn, Reg};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

/// What a memory access needs set up: its base register, its index register (if any), its size.
fn memory_operands(insn: &Insn) -> Option<(u8, Option<u8>, u32)> {
    let from_addr = |addr: Addr, bytes: u32| match addr {
        Addr::Offset { rn, .. } | Addr::PreIndex { rn, .. } | Addr::PostIndex { rn, .. } => Some((rn.0, None, bytes)),
        Addr::RegOffset { rn, rm, .. } => Some((rn.0, Some(rm.0), bytes)),
        Addr::Literal { .. } => None,
    };
    match *insn {
        Insn::LoadStore { addr, size, .. } => from_addr(addr, size.bytes()),
        Insn::Pair { addr, size, .. } => from_addr(addr, 2 * size.bytes()),
        Insn::Exclusive { rn, size, .. } | Insn::Ordered { rn, size, .. } | Insn::Ldapr { rn, size, .. } => {
            Some((rn.0, None, size.bytes()))
        }
        Insn::Cas { rn, size, pair, .. } => Some((rn.0, None, if pair { 2 } else { 1 } * size.bytes())),
        Insn::Atomic { rn, size, .. } => Some((rn.0, None, size.bytes())),
        Insn::LdrPac { rn, .. } => Some((rn.0, None, 8)),
        Insn::Sys { rt, .. } => Some((rt.0, None, 64)),
        Insn::SimdLdSt { rn, what, .. } => Some((rn.0, None, what.bytes())),
        _ => None,
    }
}

/// Whether the architecture leaves the result to the implementation (CONSTRAINED UNPREDICTABLE).
fn unpredictable(insn: &Insn) -> bool {
    let writeback = |addr: Addr| match addr {
        Addr::PreIndex { rn, .. } | Addr::PostIndex { rn, .. } => Some(rn),
        _ => None,
    };
    let clash = |base: Option<Reg>, moved: &[Reg]| base.is_some_and(|b| !b.is_31() && moved.contains(&b));
    match *insn {
        // (a SIMD and FP register is no base register: only a general one can clash)
        Insn::LoadStore { addr, rt, kind, .. } => {
            !matches!(kind, a64::LsKind::LoadFp | a64::LsKind::StoreFp) && clash(writeback(addr), &[rt])
        }
        Insn::Pair { addr, rt, rt2, kind, .. } => {
            let fp = matches!(kind, a64::PairKind::LoadFp | a64::PairKind::StoreFp);
            let load = matches!(kind, a64::PairKind::Load | a64::PairKind::LoadSignedWord | a64::PairKind::LoadFp);
            (!fp && clash(writeback(addr), &[rt, rt2])) || (load && rt == rt2)
        }
        Insn::LdrPac { writeback: true, rt, rn, .. } => rn == rt && !rn.is_31(),
        Insn::Exclusive { load: false, rs, rt, rt2, rn, .. } => {
            rs == rt || (rs == rn && !rn.is_31()) || rt2.is_some_and(|r| r == rs)
        }
        Insn::Exclusive { load: true, rt, rt2: Some(rt2), .. } => rt == rt2,
        _ => false,
    }
}

/// The command; `args` are those after `words`.
pub fn run(args: &[String]) -> ExitCode {
    match words(args) {
        Ok(n) => {
            println!("longshot-builder dev words: {n} functions");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("longshot-builder dev words: {e}");
            ExitCode::FAILURE
        }
    }
}

fn words(args: &[String]) -> Result<usize, String> {
    let (mut input, mut out, mut list, mut pc, mut seq) = (None, None, None, None, None);
    let mut count_checks = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--in" => input = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--list" => list = Some(PathBuf::from(value()?)),
            "--seq" => seq = Some(PathBuf::from(value()?)),
            "--count-checks" => count_checks = true,
            "--pc" => pc = Some(u64::from_str_radix(value()?.trim_start_matches("0x"), 16).map_err(|e| e.to_string())?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let (Some(input), Some(out), Some(list), Some(pc)) = (input, out, list, pc) else {
        return Err("--in, --out, --list and --pc are required".into());
    };
    let text = std::fs::read_to_string(&input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut module =
        llvm::Module::new(llvm::Options { polls: false, mark_bitmap: false, count_checks, ..llvm::Options::default() });
    let mut listing = String::new();
    for (i, line) in text.lines().enumerate() {
        let word = u32::from_str_radix(line.trim(), 16)
            .map_err(|_| format!("{}: line {}: {line:?}", input.display(), i + 1))?;
        let at = pc + 4 * i as u64;
        let Some(mut f) = lift::single(word, at) else { continue };
        let insn = a64::decode(word).expect("a lifted word decodes");
        if unpredictable(&insn) {
            continue;
        }
        // (the passes as emit runs them, so that the check covers them too)
        passes::share_marks(&mut f);
        passes::simplify(&mut f);
        f.verify().map_err(|e| format!("{word:08x}: {e}"))?;
        module.add(&f, llvm::Runs::default());
        let literal = matches!(insn, Insn::LoadStore { addr: Addr::Literal { .. }, .. });
        let _ = match memory_operands(&insn) {
            Some((rn, rm, bytes)) => {
                let rm = rm.map_or("-".to_string(), |r| r.to_string());
                writeln!(listing, "{i} {word:08x} mem {rn} {rm} {bytes} {insn}")
            }
            None if literal => writeln!(listing, "{i} {word:08x} lit - - - {insn}"),
            None => writeln!(listing, "{i} {word:08x} alu - - - {insn}"),
        };
    }
    if let Some(seq) = seq {
        let text = std::fs::read_to_string(&seq).map_err(|e| format!("{}: {e}", seq.display()))?;
        for (i, line) in text.lines().enumerate() {
            let bad = || format!("{}: line {}: {line:?}", seq.display(), i + 1);
            let mut fields = line.split_whitespace();
            let at = u64::from_str_radix(fields.next().unwrap_or_default(), 16).map_err(|_| bad())?;
            let words = fields.map(|t| u32::from_str_radix(t, 16).map_err(|_| bad())).collect::<Result<Vec<_>, _>>()?;
            let mut f = lift::straight(&words, at).ok_or_else(|| format!("{}: a word does not lift", bad()))?;
            passes::share_marks(&mut f);
            passes::simplify(&mut f);
            f.verify().map_err(|e| format!("sequence at {at:x}: {e}"))?;
            module.add(&f, llvm::Runs::default());
        }
    }
    let n = module.len();
    std::fs::write(&out, module.finish()).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::write(&list, listing).map_err(|e| format!("{}: {e}", list.display()))?;
    Ok(n)
}

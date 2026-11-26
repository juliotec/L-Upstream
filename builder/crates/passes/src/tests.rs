//! The passes on functions lowered from assembled instructions (none from the game).

use super::*;
use mir::{BinOp, Function, Loc, Op, Term, Type};

const PC: u64 = 0x8000_0000;
const STR_X1_X0: u32 = 0xf900_0001; // str x1, [x0]
const STR_X2_X0_8: u32 = 0xf900_0402; // str x2, [x0, #8]
const STR_X1_X0_4096: u32 = 0xf908_0001; // str x1, [x0, #4096]
const ADD_X0_1: u32 = 0x9100_0400; // add x0, x0, #1
const STLR_X1_X0: u32 = 0xc89f_fc01; // stlr x1, [x0]
const STR_X1_SP: u32 = 0xf900_03e1; // str x1, [sp]

/// One block of these words (a `None` is a call), then a return.
fn lowered(words: &[Option<u32>]) -> Function {
    let mut f = Function::new(PC);
    let b = f.start;
    for (i, w) in words.iter().enumerate() {
        match w {
            Some(w) => {
                let insn = a64::decode(*w).expect("an instruction");
                sem::lower(&mut f, b, PC + 4 * i as u64, &insn).expect("lowered");
            }
            None => {
                f.push(b, Op::Call { target: PC + 0x1000 });
            }
        }
    }
    f.terminate(b, Term::Return);
    f
}

/// Each mark's size, in order.
fn marks(f: &Function) -> Vec<u32> {
    f.block(f.start)
        .insts
        .iter()
        .filter_map(|&v| match f.inst(v).op {
            Op::Mark { bytes, .. } => Some(bytes),
            _ => None,
        })
        .collect()
}

#[test]
fn stores_through_one_register_share_a_mark_over_their_span() {
    let mut f = lowered(&[Some(STR_X1_X0), Some(STR_X2_X0_8)]);
    assert_eq!(marks(&f), [8, 8]);
    let st = share_marks(&mut f);
    f.verify().unwrap();
    assert_eq!(marks(&f), [16], "{f}");
    assert_eq!(st, MarkStats { stores: 2, groups: 1, grouped: 2 });
}

#[test]
fn the_register_written_between_ends_the_group() {
    let mut f = lowered(&[Some(STR_X1_X0), Some(ADD_X0_1), Some(STR_X2_X0_8)]);
    share_marks(&mut f);
    f.verify().unwrap();
    assert_eq!(marks(&f), [8, 8], "{f}");
}

#[test]
fn a_register_s_old_value_used_after_its_write_marks_from_itself() {
    // (x0's value A stored through, x0 written, its new value B stored through, then A again (forwarded, as `mov x1,
    // x0` before the write would leave it): A's stores share a mark from A, B's keeps its own; none from B covers A's)
    let mut f = Function::new(PC);
    let b = f.start;
    let store = |f: &mut Function, addr: mir::Value| {
        let x = f.push(b, Op::Get(Loc::X(1)));
        f.push(b, Op::Store { addr, value: x, space: mir::Space::Guest, order: None });
        f.push(b, Op::Mark { addr, bytes: 8 });
    };
    let a = f.push(b, Op::Get(Loc::X(0)));
    store(&mut f, a);
    let k = f.push(b, Op::Const { ty: Type::I64, value: 0x10_0000 });
    let moved = f.push(b, Op::Bin(BinOp::Add, a, k));
    f.push(b, Op::Set(Loc::X(0), moved));
    let bb = f.push(b, Op::Get(Loc::X(0)));
    store(&mut f, bb);
    let eight = f.push(b, Op::Const { ty: Type::I64, value: 8 });
    let a8 = f.push(b, Op::Bin(BinOp::Add, a, eight));
    store(&mut f, a8);
    f.terminate(b, Term::Return);
    f.verify().unwrap();
    share_marks(&mut f);
    f.verify().unwrap();
    let bases: Vec<(mir::Value, u32)> = f
        .block(b)
        .insts
        .iter()
        .filter_map(|&v| match f.inst(v).op {
            Op::Mark { addr, bytes } => Some((addr, bytes)),
            _ => None,
        })
        .collect();
    assert!(bases.contains(&(a, 16)), "A's two stores share a mark from A: {f}");
    assert!(bases.contains(&(bb, 8)), "B's keeps its own: {f}");
    assert_eq!(bases.len(), 2, "{f}");
}

#[test]
fn a_call_between_ends_the_run() {
    let mut f = lowered(&[Some(STR_X1_X0), None, Some(STR_X2_X0_8)]);
    share_marks(&mut f);
    assert_eq!(marks(&f), [8, 8], "{f}");
}

#[test]
fn a_span_over_4_kb_keeps_its_marks() {
    let mut f = lowered(&[Some(STR_X1_X0), Some(STR_X1_X0_4096)]);
    share_marks(&mut f);
    assert_eq!(marks(&f), [8, 8], "{f}");
}

#[test]
fn atomic_stores_keep_their_own_marks_and_sp_stores_have_none() {
    let mut f = lowered(&[Some(STR_X1_X0), Some(STLR_X1_X0), Some(STR_X2_X0_8), Some(STR_X1_SP)]);
    share_marks(&mut f);
    f.verify().unwrap();
    // (the STLR ends the run: x0's first store alone before it, the third after)
    assert_eq!(marks(&f), [8, 8, 8], "{f}");
}

#[test]
fn the_shared_mark_goes_before_the_block_s_end_and_before_a_branch() {
    let mut f = lowered(&[Some(STR_X2_X0_8), Some(STR_X1_X0)]);
    share_marks(&mut f);
    f.verify().unwrap();
    let last = *f.block(f.start).insts.last().unwrap();
    assert!(matches!(f.inst(last).op, Op::Mark { bytes: 16, .. }), "{f}");
}

const ADDS_X0_X0_1: u32 = 0xb100_0400; // adds x0, x0, #1
const ADD_X1_X0_1: u32 = 0x9100_0401; // add x1, x0, #1

fn count(f: &Function, pred: impl Fn(&Op) -> bool) -> usize {
    f.blocks().map(|(_, b)| b.insts.iter().filter(|&&v| pred(&f.inst(v).op)).count()).sum()
}

#[test]
fn a_location_read_after_its_write_is_the_written_value() {
    let mut f = lowered(&[Some(ADD_X0_1), Some(ADD_X1_X0_1)]);
    assert_eq!(count(&f, |op| matches!(op, Op::Get(mir::Loc::X(0)))), 2);
    let n = forward(&mut f);
    f.verify().unwrap();
    assert_eq!(n, 1, "{f}");
    assert_eq!(count(&f, |op| matches!(op, Op::Get(mir::Loc::X(0)))), 1, "{f}");
}

#[test]
fn flags_written_again_before_any_read_are_dead() {
    let mut f = lowered(&[Some(ADDS_X0_X0_1), Some(ADDS_X0_X0_1)]);
    let st = simplify(&mut f);
    f.verify().unwrap();
    // (the first ADDS's four flags and its X0 are written again before anything reads them; the second's leave by
    // the return)
    assert_eq!(st.dead_writes, 5, "{f}");
    assert_eq!(count(&f, |op| matches!(op, Op::Set(mir::Loc::Flag(_), _))), 4, "{f}");
}

#[test]
fn a_call_reads_every_location() {
    let mut f = lowered(&[Some(ADDS_X0_X0_1), None, Some(ADDS_X0_X0_1)]);
    let st = simplify(&mut f);
    f.verify().unwrap();
    assert_eq!(st.dead_writes, 0, "{f}");
    assert_eq!(st.forwarded, 0, "{f}");
}

#[test]
fn a_write_read_in_another_block_lives() {
    // b0: adds x0, x0, #1; jump b1.  b1: x1 = x0 + 1; adds; return
    let mut f = Function::new(PC);
    let b0 = f.start;
    let b1 = f.add_block(PC + 4);
    sem::lower(&mut f, b0, PC, &a64::decode(ADDS_X0_X0_1).unwrap()).unwrap();
    f.terminate(b0, Term::Jump(mir::Edge::new(mir::Dest::Block(b1), false)));
    sem::lower(&mut f, b1, PC + 4, &a64::decode(ADD_X1_X0_1).unwrap()).unwrap();
    sem::lower(&mut f, b1, PC + 8, &a64::decode(ADDS_X0_X0_1).unwrap()).unwrap();
    f.terminate(b1, Term::Return);
    let st = simplify(&mut f);
    f.verify().unwrap();
    // (b0's X0 is read in b1; its flags are written again there before any read)
    assert_eq!(st.dead_writes, 4, "{f}");
    assert_eq!(count(&f, |op| matches!(op, Op::Set(mir::Loc::X(0), _))), 2, "{f}");
}

const FADD_S0_S1_S2: u32 = 0x1e22_2820; // fadd s0, s1, s2
const FMUL_S3_S0_S2: u32 = 0x1e22_0803; // fmul s3, s0, s2

#[test]
fn the_fpcr_and_register_reads_of_float_instructions_are_shared() {
    let mut f = lowered(&[Some(FADD_S0_S1_S2), Some(FMUL_S3_S0_S2)]);
    let checked =
        |f: &Function| f.block(f.start).insts.iter().filter(|&&v| matches!(f.inst(v).op, Op::Checked { .. })).count();
    let st = simplify(&mut f);
    f.verify().unwrap();
    assert!(st.shared > 0, "{st:?}");
    assert_eq!(checked(&f), 2, "{f}");
    // (one FPCR read for both instructions' helpers, and V2's lane read once: forwarded, then shared)
    let count =
        |f: &Function, want: fn(&Op) -> bool| f.block(f.start).insts.iter().filter(|&&v| want(&f.inst(v).op)).count();
    assert_eq!(count(&f, |op| matches!(op, Op::Get(mir::Loc::Fpcr))), 1, "{f}");
    assert_eq!(count(&f, |op| matches!(op, Op::Get(mir::Loc::V(2)))), 1, "{f}");
    assert_eq!(f.helpers().count(), 2, "{f}");
}

#[test]
fn a_float_instruction_does_not_end_a_run_of_marks() {
    let mut f = lowered(&[Some(STR_X1_X0), Some(FADD_S0_S1_S2), Some(STR_X2_X0_8)]);
    share_marks(&mut f);
    f.verify().unwrap();
    assert_eq!(marks(&f), [16], "{f}");
}

/// A chain of `n` blocks, each setting X0 and going to the next, the last one returning; the third goes back to the
/// first (a loop).
fn chain(n: u32) -> Function {
    let mut f = Function::new(0x8000_0000);
    let blocks: Vec<mir::BlockId> =
        (0..n).map(|i| if i == 0 { f.start } else { f.add_block(0x8000_0000 + 4 * u64::from(i)) }).collect();
    for (i, &b) in blocks.iter().enumerate() {
        let k = f.push(b, Op::Const { ty: mir::Type::I64, value: i as u128 });
        f.push(b, Op::Set(mir::Loc::X(0), k));
        let term = match blocks.get(i + 1) {
            Some(&next) if i == 2 => {
                let z = f.push(b, Op::Get(mir::Loc::Flag(mir::Flag::Z)));
                let back = mir::Edge::new(mir::Dest::Block(blocks[0]), true);
                Term::Branch { cond: z, then: back, otherwise: mir::Edge::new(mir::Dest::Block(next), false) }
            }
            Some(&next) => Term::Jump(mir::Edge::new(mir::Dest::Block(next), false)),
            None => Term::Return,
        };
        f.terminate(b, term);
    }
    f.verify().unwrap();
    f
}

#[test]
fn a_small_function_is_not_split() {
    let f = chain(8);
    assert_eq!(split(f.clone(), SplitOptions::default()), vec![f]);
}

#[test]
fn a_split_function_is_a_trampoline_and_pieces_that_go_to_each_other() {
    let f = chain(12);
    let pieces = split(f, SplitOptions { over: 1, piece: 9 });
    assert!(pieces.len() >= 3, "a trampoline and two pieces at least: {}", pieces.len());
    let t = &pieces[0];
    assert_eq!(t.name(), "f_80000000");
    assert!(t.wrapper && t.block_count() == 1);
    assert!(matches!(
        t.block(t.start).term,
        Some(Term::Jump(mir::Edge { to: mir::Dest::Piece { piece: 1, entry: 0 }, .. }))
    ));
    for (i, p) in pieces.iter().enumerate().skip(1) {
        p.verify().unwrap_or_else(|e| panic!("{e}\n{p}"));
        assert_eq!(p.name(), format!("f_80000000.p{i}"));
        assert!(!p.entries.is_empty(), "every piece is entered: {p}");
    }
    mir::verify_pieces(&pieces).unwrap();
    // (every block is in one piece, and the loop's back edge, a poll, keeps its poll across a cut)
    let blocks: usize = pieces[1..].iter().map(Function::block_count).sum();
    assert_eq!(blocks, 12);
    let polls = pieces[1..]
        .iter()
        .flat_map(|p| p.blocks().flat_map(|(_, b)| b.term.as_ref().unwrap().edges()))
        .filter(|e| e.poll)
        .count();
    assert_eq!(polls, 1);
}

#[test]
fn a_cut_goes_where_fewest_edges_cross_and_not_through_a_loop() {
    // (the loop over the first three blocks stays in one piece: its back edge weighs more than a cut elsewhere)
    let f = chain(12);
    let pieces = split(f, SplitOptions { over: 1, piece: 12 });
    let first = &pieces[1];
    let pcs: Vec<u64> = first.blocks().map(|(_, b)| b.pc).collect();
    assert!(pcs.contains(&0x8000_0000) && pcs.contains(&0x8000_0008), "{first}");
    assert!(cost(&chain(12)) > 12);
}

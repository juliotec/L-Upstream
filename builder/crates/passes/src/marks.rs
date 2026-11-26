//! Shared page marks: stores near each other through one base share one mark.
//!
//! Every plain store to guest memory is followed by its [`Op::Mark`]: the pages of its first and its last byte are
//! marked written, for the GPU's copies of guest memory. Stores through one value of a base register, at constant
//! offsets from it, in one straight run (no call, SVC, fence or atomic between them), mark at most
//! two pages between them when they span 4 KB or less: one mark over the whole span, where the run ends, does what
//! theirs did. Most of a function's stores are such runs (a structure filled in, registers saved to a frame).
//!
//! What a mark is for decides where the shared one may go. The GPU reads the marks when the guest submits work or
//! signals it (an SVC, a call into the host), never in the middle of a run, so a mark moved to the run's end (still
//! before any call, SVC, branch or the register's next value) is seen when every one of the stores' marks would have
//! been. Stores through SP mark nothing to begin with; atomic and exclusive stores keep their own marks.

use mir::{BinOp, BlockId, Function, Loc, Op, Space, Type, Value};
use std::collections::{BTreeMap, HashMap};

/// The widest span one shared mark covers: wider and a page between its first and last could go unmarked.
const SPAN: i64 = 4096;

/// What [`share_marks`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkStats {
    /// Plain stores through a base register that it looked at.
    pub stores: usize,
    /// Groups of two or more that now share one mark.
    pub groups: usize,
    /// The stores of those groups.
    pub grouped: usize,
}

impl std::ops::AddAssign for MarkStats {
    fn add_assign(&mut self, o: MarkStats) {
        self.stores += o.stores;
        self.groups += o.groups;
        self.grouped += o.grouped;
    }
}

/// `f` with the page marks of each group of stores shared; what it did.
pub fn share_marks(f: &mut Function) -> MarkStats {
    let mut stats = MarkStats::default();
    for b in 0..f.block_count() {
        stats += share_in(f, BlockId(b as u32));
    }
    stats
}

/// Where a value points: `base` (a read of X`reg` as it was at `epoch`) plus `offset`.
#[derive(Clone, Copy)]
struct Origin {
    reg: u8,
    epoch: u32,
    base: Value,
    offset: i64,
}

/// The marks of the stores through one value of a register (a register between two of its writes) in the current
/// run.
struct Group {
    base: Value,
    /// (the mark's place in the block, its first byte's offset, its bytes)
    marks: Vec<(usize, i64, u32)>,
}

/// One mark to write before the instruction at a place of the block: `bytes` from `base + offset`.
struct Shared {
    base: Value,
    offset: i64,
    bytes: u32,
}

struct Run {
    /// By register and its value's epoch: a value of a register still used after the register's next write (the
    /// old value, forwarded) is a group of its own, from its own base.
    groups: BTreeMap<(u8, u32), Group>,
    dropped: Vec<bool>,
    before: BTreeMap<usize, Vec<Shared>>,
    stats: MarkStats,
}

impl Run {
    /// The group of `key` done, its shared mark (if it has one) placed before instruction `at`.
    fn flush(&mut self, key: (u8, u32), at: usize) {
        let Some(g) = self.groups.remove(&key) else { return };
        self.stats.stores += g.marks.len();
        let lo = g.marks.iter().map(|&(_, o, _)| o).min().expect("a group has a mark");
        let hi = g.marks.iter().map(|&(_, o, n)| o + i64::from(n)).max().expect("a group has a mark");
        if g.marks.len() < 2 || hi - lo > SPAN {
            return;
        }
        for &(i, _, _) in &g.marks {
            self.dropped[i] = true;
        }
        let bytes = u32::try_from(hi - lo).expect("a span of 4 KB at most");
        self.before.entry(at).or_default().push(Shared { base: g.base, offset: lo, bytes });
        self.stats.groups += 1;
        self.stats.grouped += g.marks.len();
    }

    fn flush_all(&mut self, at: usize) {
        let keys: Vec<(u8, u32)> = self.groups.keys().copied().collect();
        for k in keys {
            self.flush(k, at);
        }
    }
}

fn share_in(f: &mut Function, b: BlockId) -> MarkStats {
    let insts = f.block(b).insts.clone();
    let mut run = Run {
        groups: BTreeMap::new(),
        dropped: vec![false; insts.len()],
        before: BTreeMap::new(),
        stats: MarkStats::default(),
    };
    let mut epoch = [0u32; 31];
    let mut origin: HashMap<Value, Origin> = HashMap::new();
    let constant = |f: &Function, v: Value| match f.inst(v).op {
        Op::Const { ty: Type::I64, value } => Some(value as u64 as i64),
        _ => None,
    };

    for (i, &v) in insts.iter().enumerate() {
        match f.inst(v).op {
            Op::Get(Loc::X(reg)) => {
                origin.insert(v, Origin { reg, epoch: epoch[usize::from(reg)], base: v, offset: 0 });
            }
            Op::Bin(BinOp::Add, a, k) => {
                if let (Some(o), Some(k)) = (origin.get(&a).copied(), constant(f, k)) {
                    origin.insert(v, Origin { offset: o.offset.wrapping_add(k), ..o });
                }
            }
            Op::Set(Loc::X(reg), _) => {
                // (the register's next value: its reads from here are another group's; the old value's group goes on,
                // its base still the value it was)
                epoch[usize::from(reg)] += 1;
            }
            Op::Mark { addr, bytes } => {
                let plain_store = i > 0
                    && matches!(f.inst(insts[i - 1]).op,
                        Op::Store { addr: a, space: Space::Guest, order: None, .. } if a == addr);
                match origin.get(&addr).copied() {
                    Some(o) if plain_store => {
                        let g = run.groups.entry((o.reg, o.epoch)).or_insert(Group { base: o.base, marks: Vec::new() });
                        g.marks.push((i, o.offset, bytes));
                    }
                    _ => {} // (an address the pass does not follow, or an atomic's mark: it keeps its own)
                }
            }
            // (what a run may hold: pure ops, a fast path's among them (its helper touches no memory and is no call
            // into the host), the state's reads and writes, plain loads and stores)
            ref op if op.is_pure() => {}
            Op::Get(_) | Op::Set(..) | Op::Load { order: None, .. } | Op::Store { order: None, .. } => {}
            // (calls, SVCs, fences, atomics and exclusives end the run: its marks go before them)
            _ => run.flush_all(i),
        }
    }
    run.flush_all(insts.len());
    if run.stats.groups == 0 {
        return run.stats;
    }

    let mut out = Vec::with_capacity(insts.len());
    for i in 0..=insts.len() {
        for s in run.before.remove(&i).unwrap_or_default() {
            let addr = if s.offset == 0 {
                s.base
            } else {
                let k = f.detached(Op::Const { ty: Type::I64, value: u128::from(s.offset as u64) });
                let a = f.detached(Op::Bin(BinOp::Add, s.base, k));
                out.extend([k, a]);
                a
            };
            out.push(f.detached(Op::Mark { addr, bytes: s.bytes }));
        }
        if i < insts.len() && !run.dropped[i] {
            out.push(insts[i]);
        }
    }
    f.set_insts(b, out);
    run.stats
}

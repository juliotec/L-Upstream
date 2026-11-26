//! A function too big for the compiler, split into pieces it compiles in reasonable time.
//!
//! clang's time on one function grows faster than its size (register allocation, the loop passes over one flow graph
//! of tens of thousands of blocks): a single guest function of 25 MB of IR took a unit longer than all the others.
//! [`split`] cuts such a function at block boundaries into pieces, each a function of its own. The mid IR makes the
//! cut exact: a value lives in its own block, so what flows between blocks is guest state alone, and an edge from one
//! piece to another goes the way a tail call does: the state that may differ from the context is written to it, the
//! entry block's index is put in the context (`Ctx.piece`), and the other piece is entered by `musttail`, reloading
//! what it reads (its entry reads what any of its entry blocks reads). The guest's call depth is unchanged.
//!
//! The function keeps its symbol, a trampoline that goes to the first piece at the function's start; the pieces are
//! `<symbol>.p<n>`, internal to the module.
//!
//! **Where it cuts.** The blocks in address order (an edit's blocks beside their guest block), cut into runs of about
//! [`SplitOptions::piece`] of [`cost`], each cut placed, within half a piece of that, where the fewest edges cross it,
//! an edge back (a loop's) counting [`BACK_WEIGHT`] times a forward one: a loop cut in two hops between pieces each
//! time round, where a forward edge hops once.

use mir::{BlockId, Dest, Edge, Function, Op, Pieces, Term, Value};
use std::collections::HashMap;

/// How much more a cut through a loop's back edge costs than one through a forward edge.
pub const BACK_WEIGHT: u64 = 8;

/// What an instruction that hands the state to the context and back (a call, an SVC, a host call) costs, in
/// instructions: the writer moves each location it syncs there with a load, an address and a store, both ways.
pub const SYNC_WEIGHT: usize = 40;

/// A function's estimated size as the compiler sees it: its instructions, its blocks, and [`SYNC_WEIGHT`] for each that
/// syncs the state (a function of many calls is mostly the state's moves around them).
pub fn cost(f: &Function) -> usize {
    f.blocks().map(|(_, b)| block_cost(f, b)).sum()
}

fn block_cost(f: &Function, b: &mir::Block) -> usize {
    1 + b.insts.iter().map(|&v| if f.inst(v).op.exposes_state() { SYNC_WEIGHT } else { 1 }).sum::<usize>()
}

/// When and how a function is split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitOptions {
    /// A function whose [`cost`] is more than this is split (0: none is).
    pub over: usize,
    /// About how much of it a piece holds.
    pub piece: usize,
}

impl Default for SplitOptions {
    fn default() -> SplitOptions {
        SplitOptions { over: 60_000, piece: 20_000 }
    }
}

/// `f` as the functions it is written as: itself when it is not over [`SplitOptions::over`], else its trampoline
/// (its own symbol) and its pieces.
pub fn split(f: Function, o: SplitOptions) -> Vec<Function> {
    if o.over == 0 || f.block_count() < 2 || f.pieces.is_some() || cost(&f) <= o.over {
        return vec![f];
    }
    let order = order(&f);
    let cuts = cuts(&f, &order, o.piece.max(1));
    if cuts.is_empty() {
        return vec![f];
    }
    // (each block's piece, 1 on, and its index among the blocks the piece is entered at)
    let mut piece_of: HashMap<BlockId, u32> = HashMap::new();
    let mut p = 1u32;
    let mut next = cuts.iter().peekable();
    for (i, &b) in order.iter().enumerate() {
        if next.peek() == Some(&&i) {
            next.next();
            p += 1;
        }
        piece_of.insert(b, p);
    }
    let count = p;
    let mut entries: Vec<Vec<BlockId>> = vec![Vec::new(); count as usize + 1];
    let enter = |b: BlockId, entries: &mut Vec<Vec<BlockId>>| -> u32 {
        let list = &mut entries[piece_of[&b] as usize];
        match list.iter().position(|&x| x == b) {
            Some(i) => i as u32,
            None => {
                list.push(b);
                list.len() as u32 - 1
            }
        }
    };
    let start = enter(f.start, &mut entries);
    for (b, block) in f.blocks() {
        for e in block.term.as_ref().expect("a verified function").edges() {
            if let Dest::Block(t) = e.to
                && piece_of[&t] != piece_of[&b]
            {
                enter(t, &mut entries);
            }
        }
    }
    let base = f.name();
    let mut out = Vec::with_capacity(count as usize + 1);
    out.push(trampoline(&f, &base, piece_of[&f.start], start));
    for p in 1..=count {
        let blocks: Vec<BlockId> = order.iter().copied().filter(|b| piece_of[b] == p).collect();
        out.push(piece(&f, &base, p, &blocks, &entries, &piece_of));
    }
    out
}

/// The blocks in address order, an edit's blocks (which carry their guest block's address) after it.
fn order(f: &Function) -> Vec<BlockId> {
    let mut order: Vec<BlockId> = f.blocks().map(|(id, _)| id).collect();
    order.sort_by_key(|&b| (f.block(b).pc, b));
    order
}

/// Where the pieces start: indexes into `order`, ascending (none: no cut worth making).
fn cuts(f: &Function, order: &[BlockId], piece: usize) -> Vec<usize> {
    let n = order.len();
    let pos: HashMap<BlockId, usize> = order.iter().enumerate().map(|(i, &b)| (b, i)).collect();
    // (crossing[i]: the weight of the edges over the cut before block i)
    let mut diff = vec![0i64; n + 1];
    for (b, block) in f.blocks() {
        for e in block.term.as_ref().expect("a verified function").edges() {
            if let Dest::Block(t) = e.to {
                let (u, v) = (pos[&b], pos[&t]);
                let w = if v <= u { BACK_WEIGHT } else { 1 } as i64;
                let (lo, hi) = (u.min(v), u.max(v));
                diff[lo + 1] += w;
                diff[hi + 1] -= w;
            }
        }
    }
    let mut crossing = vec![0i64; n + 1];
    let mut run = 0;
    for i in 0..=n {
        run += diff[i];
        crossing[i] = run;
    }
    // (sizes[i]: the instructions before block i)
    let mut sizes = vec![0usize; n + 1];
    for (i, &b) in order.iter().enumerate() {
        sizes[i + 1] = sizes[i] + block_cost(f, f.block(b));
    }
    let total = sizes[n];
    let mut out = Vec::new();
    let mut from = 0;
    while total - sizes[from] > piece + piece / 2 {
        let (lo, hi) = (sizes[from] + piece / 2, sizes[from] + piece + piece / 2);
        let target = sizes[from] + piece;
        let best = (from + 1..n)
            .take_while(|&i| sizes[i] <= hi)
            .filter(|&i| sizes[i] >= lo)
            .min_by_key(|&i| (crossing[i], sizes[i].abs_diff(target)));
        let Some(cut) = best.or_else(|| (from + 1..n).find(|&i| sizes[i] >= target)) else { break };
        out.push(cut);
        from = cut;
    }
    out
}

/// The function's own symbol: straight to its start, in the piece that holds it.
fn trampoline(f: &Function, base: &str, piece: u32, entry: u32) -> Function {
    let mut t = Function::new(f.entry);
    if let Some(s) = f.symbol() {
        t.set_symbol(s);
    }
    t.wrapper = true;
    t.pieces = Some(Pieces { base: base.to_string(), index: 0 });
    let b = t.start;
    t.terminate(b, Term::Jump(Edge::new(Dest::Piece { piece, entry }, false)));
    t
}

/// Piece `p`: `blocks` of `f`, its edges to the other pieces' blocks through their entries.
fn piece(
    f: &Function,
    base: &str,
    p: u32,
    blocks: &[BlockId],
    entries: &[Vec<BlockId>],
    piece_of: &HashMap<BlockId, u32>,
) -> Function {
    let mut g = Function::new(f.entry);
    g.set_symbol(Pieces::symbol(base, p));
    g.pieces = Some(Pieces { base: base.to_string(), index: p });
    for (_, h) in f.helpers() {
        g.add_helper(h.clone());
    }
    // (the blocks, the first one the function's start block, which Function::new made)
    let mut map: HashMap<BlockId, BlockId> = HashMap::new();
    for (i, &b) in blocks.iter().enumerate() {
        let old = f.block(b);
        let nb = if i == 0 { g.start } else { g.add_block(old.pc) };
        g.set_guest_start(nb, old.guest_start);
        g.set_guest_term(nb, old.guest_term);
        map.insert(b, nb);
    }
    let edge = |e: Edge| -> Edge {
        match e.to {
            Dest::Block(t) if piece_of[&t] == p => Edge::new(Dest::Block(map[&t]), e.poll),
            Dest::Block(t) => {
                let q = piece_of[&t];
                let entry =
                    entries[q as usize].iter().position(|&x| x == t).expect("every crossing target is an entry");
                Edge::new(Dest::Piece { piece: q, entry: entry as u32 }, e.poll)
            }
            other => Edge::new(other, e.poll),
        }
    };
    for &b in blocks {
        let old = f.block(b);
        let nb = map[&b];
        let mut values: HashMap<Value, Value> = HashMap::new();
        let mut insts = Vec::with_capacity(old.insts.len());
        for &v in &old.insts {
            let mut op = f.inst(v).op.clone();
            for x in op.operands_mut() {
                *x = values[x];
            }
            if let Op::Checked { helper, .. } = &mut op {
                *helper = g.helper_named(&f.helper(*helper).name).expect("the helpers copied");
            }
            let nv = g.detached(op);
            values.insert(v, nv);
            insts.push(nv);
        }
        g.set_insts(nb, insts);
        let mut term = old.term.clone().expect("a verified function");
        if let Some(x) = term.operand_mut() {
            *x = values[x];
        }
        let term = match term {
            Term::Jump(e) => Term::Jump(edge(e)),
            Term::Branch { cond, then, otherwise } => {
                Term::Branch { cond, then: edge(then), otherwise: edge(otherwise) }
            }
            Term::Switch { value, cases } => {
                Term::Switch { value, cases: cases.into_iter().map(|(a, e)| (a, edge(e))).collect() }
            }
            t @ (Term::Return | Term::IndirectJump(_) | Term::Trap { .. }) => t,
        };
        g.terminate(nb, term);
    }
    g.entries = entries[p as usize].iter().map(|b| map[b]).collect();
    g
}

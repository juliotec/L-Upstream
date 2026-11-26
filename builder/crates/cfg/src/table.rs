//! Jump tables: what a BR's register holds, found by running the straight-line code before it symbolically, and the
//! searches that finds the table address with (an ADR table address too, a table register reused between the load and
//! the BR, the window ended at an authenticated return).
//!
//! LLVM's AArch64 tables: the target is `base + (entry << k)`, the entry loaded from a constant table. The table's
//! address may be set before the window (found through the code's predecessors, or the function's one assignment),
//! the bound may be on a copy of the index, or there may be none (entries read until one leaves the function).

use crate::classify::{Kind, add_imm, adr, adrp, pair_offset, writes};
use crate::walk::{Discovery, Preds};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// What the symbolic window knows a register holds.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Val {
    /// A constant.
    Const(u64),
    /// The register's value from before the window.
    Reg(u32),
    /// A register from before the window plus an immediate.
    RegOff(u32, u64),
    /// A load from `base + (index << shift)` of `size` bytes (signed or not), at `at`.
    Load { base: Box<Val>, index: u32, size: u32, signed: bool, at: u64 },
    /// `a + (b << k)`, either side unknown.
    Add(Option<Box<Val>>, Option<Box<Val>>, u32),
}

/// The window's registers. A register is absent (not written in the window: its value from before), known, or
/// unknown (written with something the window does not follow).
#[derive(Default)]
struct Regs(HashMap<u32, Option<Val>>);

impl Regs {
    /// What `n` holds, absent counted as its value from before the window.
    fn or_reg(&self, n: u32) -> Option<Val> {
        match self.0.get(&n) {
            None => Some(Val::Reg(n)),
            Some(v) => v.clone(),
        }
    }
    /// What `n` holds, absent counted as unknown.
    fn known(&self, n: u32) -> Option<Val> {
        self.0.get(&n).cloned().flatten()
    }
}

/// The window's index bounds: registers whose value can index this many entries and the last compare, both of the
/// current block; and the general bound, the window's last compare and the count the first B.HI or B.HS after it
/// gives, which a block's end does not forget.
#[derive(Default)]
struct Bounds {
    regs: HashMap<u32, u64>,
    cmp: Option<(u32, u64)>,
    general: Option<Bound>,
}

impl Bounds {
    /// A block ends: what it bounded is forgotten (the general bound is not).
    fn end_block(&mut self) {
        self.regs.clear();
        self.cmp = None;
    }
}

/// A compare's immediate, and the count a B.HI or B.HS after it gives.
struct Bound {
    imm: u64,
    count: Option<u64>,
}

fn is_cmp_imm(w: u32) -> bool {
    w & 0x7f80_001f == 0x7100_001f
}
fn is_ccmp_imm(w: u32) -> bool {
    w & 0x7fe0_0c10 == 0x7a40_0800
}
fn is_bhi_bhs(w: u32) -> bool {
    matches!(w & 0xff00_001f, 0x5400_0008 | 0x5400_0002)
}
/// B, RET, BR (not the authenticated forms): what follows is reached from elsewhere.
fn ends_block(w: u32) -> bool {
    w & 0xfc00_0000 == 0x1400_0000 || matches!(w & 0xffff_fc1f, 0xd65f_0000 | 0xd61f_0000)
}
/// RETAA, RETAB, BRAAZ, BRABZ, BRAA, BRAB.
fn auth_end(w: u32) -> bool {
    w == 0xd65f_0bff || w == 0xd65f_0fff || w & 0xffff_f81f == 0xd61f_081f || w & 0xffff_f800 == 0xd71f_0800
}
/// An integer load with a register offset (LDR*, not a store).
fn is_ldr_reg(w: u32) -> bool {
    w & 0x3b20_0c00 == 0x3820_0800 && (w >> 26) & 1 == 0
}

/// A function's edges for [`Discovery::index_bound`]: its predecessors, its start, and its BRs whose targets are not known
/// yet (the edges the predecessors lack)
struct Flow<'a> {
    preds: &'a Preds,
    start: u64,
    open: &'a [u64],
}

/// Where a search for a hoisted table register starts: at the table's load, when the window writes the register again
/// after it, else at the BR.
#[derive(Clone, Copy)]
struct SearchFrom {
    br_pc: u64,
    load: Option<(u64, u32)>,
}

impl SearchFrom {
    fn at(self, reg: u32) -> u64 {
        match self.load {
            Some((pc, treg)) if treg == reg => pc,
            _ => self.br_pc,
        }
    }
}

/// A place the predecessors' search follows: a register, or a stack slot (bytes from SP).
#[derive(Clone, Copy)]
enum Loc {
    Reg(u32),
    Slot(i64),
}

const WINDOW: u64 = 32;
const MAX_EXTENT: u64 = 0x40000;

impl Discovery<'_> {
    fn word(&self, pc: u64) -> u32 {
        self.image.word(pc).unwrap_or(0)
    }

    /// The targets of the BR at `br_pc` through register `reg`, if they can be found (the window cut at an
    /// authenticated return, the table register searched for at its load).
    pub(crate) fn jump_table(&mut self, br_pc: u64, reg: u32) -> Option<Vec<u64>> {
        let mut window = WINDOW;
        for k in 1..=WINDOW {
            let Some(pc) = br_pc.checked_sub(4 * k) else { break };
            if self.image.in_code(pc) && auth_end(self.word(pc)) {
                window = k - 1;
                self.stats.bump("jt_window_cut_at_auth_ret");
                break;
            }
        }
        let load = self.table_load(br_pc, window);
        let out = self.window_table(br_pc, reg, window, SearchFrom { br_pc, load });
        if load.is_some() && out.is_some() {
            self.stats.bump("jt_table_reg_reused");
        }
        out
    }

    /// (pc, base register) of the last register-offset integer load in the window, if the window writes its base
    /// register again after it.
    fn table_load(&self, br_pc: u64, window: u64) -> Option<(u64, u32)> {
        for k in 1..window {
            let pc = br_pc.checked_sub(4 * k)?;
            if !self.image.in_code(pc) {
                return None;
            }
            let w = self.word(pc);
            if ends_block(w) {
                return None;
            }
            if is_ldr_reg(w) && (w >> 22) & 3 != 0 {
                let n = (w >> 5) & 31;
                let rewritten = (pc + 4..br_pc).step_by(4).any(|q| writes(self.word(q), n));
                return rewritten.then_some((pc, n));
            }
        }
        None
    }

    fn miss(&mut self, br_pc: u64, reg: u32) {
        match self.jt_missing_at.get(&br_pc) {
            Some(&i) => self.jt_missing[i].1 = reg,
            None => {
                self.jt_missing_at.insert(br_pc, self.jt_missing.len());
                self.jt_missing.push((br_pc, reg));
            }
        }
    }

    /// The table of the BR at `br_pc` from a window of `window` instructions before it.
    fn window_table(&mut self, br_pc: u64, reg: u32, window: u64, from: SearchFrom) -> Option<Vec<u64>> {
        let mut r = Regs::default();
        let mut bounds = Bounds::default();
        let start = br_pc.saturating_sub(4 * window);
        for pc in (start..br_pc).step_by(4) {
            let w = self.word(pc);
            if !self.image.in_code(pc) || ends_block(w) {
                r.0.clear();
                bounds.end_block();
                continue;
            }
            let (d, n, m) = (w & 31, (w >> 5) & 31, (w >> 16) & 31);
            // the index bounds, beside the values
            let compare = is_cmp_imm(w) || is_ccmp_imm(w);
            let count_of = |imm: u64| if w & 15 == 8 { imm + 1 } else { imm };
            if compare {
                // (CMP's immediate shifted by 12 when its `sh` bit says so: read unshifted, the bound was 4096 times too
                // small, and the cases past it went to the switch's default, an indirect jump to no function)
                let imm = match is_cmp_imm(w) {
                    true => u64::from((w >> 10) & 0xfff) << if w & (1 << 22) != 0 { 12 } else { 0 },
                    false => u64::from((w >> 16) & 31),
                };
                bounds.cmp = Some((n, imm));
                bounds.general = Some(Bound { imm, count: None });
            } else if is_bhi_bhs(w) {
                if let Some((reg_c, imm)) = bounds.cmp {
                    bounds.regs.insert(reg_c, count_of(imm));
                }
                if let Some(b) = bounds.general.as_mut().filter(|b| b.count.is_none()) {
                    b.count = Some(count_of(b.imm));
                }
            } else if w & 0x7fe0_ffe0 == 0x2a00_03e0 {
                // (a mask's bound is not taken here: the window crosses conditional branches and their targets, and
                // runs past the table's load. index_bound finds it from the load, on every way in)
                match bounds.regs.get(&m).copied() {
                    Some(b) => bounds.regs.insert(d, b),
                    None => bounds.regs.remove(&d),
                };
            } else if d != 31 && writes(w, d) {
                bounds.regs.remove(&d);
            }
            // the values
            if let Some(page) = adrp(w, pc) {
                r.0.insert(d, Some(Val::Const(page)));
            } else if let Some(t) = adr(w, pc) {
                r.0.insert(d, Some(Val::Const(t)));
            } else if let Some(imm) = add_imm(w) {
                match r.or_reg(n) {
                    Some(Val::Const(c)) => {
                        r.0.insert(d, Some(Val::Const(c.wrapping_add(imm))));
                    }
                    Some(Val::Reg(_) | Val::RegOff(..)) if d == n => {
                        // adrp before the window, add inside it: resolved at the BR
                        r.0.remove(&d);
                    }
                    Some(Val::Reg(_)) => {
                        r.0.insert(d, Some(Val::RegOff(n, imm)));
                    }
                    _ => {
                        r.0.insert(d, None);
                    }
                }
            } else if is_ldr_reg(w) {
                // (the S bit, the index's shift, is not needed: the entry size gives it)
                let (size, opc) = (w >> 30, (w >> 22) & 3);
                let base = r.or_reg(n);
                let v = match base {
                    Some(base) if opc != 0 => {
                        Some(Val::Load { base: Box::new(base), index: m, size: 1 << size, signed: opc >= 2, at: pc })
                    }
                    _ => None,
                };
                r.0.insert(d, v);
            } else if w & 0xff20_0000 == 0x8b00_0000 && (w >> 22) & 3 == 0 {
                r.0.insert(d, Some(Val::Add(r.known(n).map(Box::new), r.known(m).map(Box::new), (w >> 10) & 63)));
            } else if w & 0xffe0_0000 == 0x8b20_0000 {
                r.0.insert(d, Some(Val::Add(r.known(n).map(Box::new), r.known(m).map(Box::new), (w >> 10) & 7)));
            } else if compare || is_bhi_bhs(w) {
                // (the bounds above; a compare's or a branch's register is no value)
            } else if writes(w, d) {
                r.0.insert(d, None);
            }
            let rt2 = (w >> 10) & 31;
            if (w >> 27) & 7 == 5 && (w >> 22) & 1 != 0 && (w >> 26) & 1 == 0 {
                // a load pair writes Rt2 too
                r.0.insert(rt2, None);
                bounds.regs.remove(&rt2);
            }
        }

        let (lo, hi) = self.function_extent(br_pc);
        let (a, b, k) = match r.known(reg) {
            Some(Val::Const(c)) => {
                // a BR to a code constant: `adr x19, label ... br x19` (a continuation in a register)
                return (lo <= c && c < hi && self.image.in_code(c)).then(|| {
                    self.stats.bump("br_constant");
                    vec![c]
                });
            }
            Some(Val::Add(a, b, k)) => (a, b, k),
            _ => return self.adr_targets(br_pc, reg),
        };
        let (a, b) = if matches!(a.as_deref(), Some(Val::Load { .. })) { (b, a) } else { (a, b) };
        let (Some(Val::Const(base)), Some(Val::Load { base: table, index, size, signed, at: load_pc })) =
            (a.as_deref(), b.as_deref())
        else {
            self.stats.bump("jt_unrecognized");
            self.miss(br_pc, reg);
            return None;
        };
        let (base, size, signed, load_pc) = (*base, *size, *signed, *load_pc);
        // (a bound on the very register that indexes the table wins; the general bound, the window's last compare, may
        // be on another register than the index's: below, the larger of it and what made the index)
        let own = bounds.regs.get(index).copied();
        let general = bounds.general.and_then(|b| b.count);
        let tables: BTreeSet<u64> = match **table {
            Val::Reg(t) | Val::RegOff(t, _) => {
                // The table address was set before the window: every ADRP(+ADD) constant reaching the BR through
                // the function's code, plus the address-order guesses; the switch takes the union of their targets.
                let off = if let Val::RegOff(_, off) = **table { off } else { 0 };
                let at = from.at(t);
                let mut found: BTreeSet<u64> = self.reaching_constants(at, t).into_iter().collect();
                let guess = self.hoisted_constant(at, t, 1024).or_else(|| self.function_constant(at, t));
                found.extend(guess);
                let found: BTreeSet<u64> = found.into_iter().map(|t| t.wrapping_add(off)).collect();
                if found.is_empty() {
                    self.stats.bump("jt_no_table");
                    self.miss(br_pc, reg);
                    return None;
                }
                if found.len() > 1 {
                    self.stats.bump("jt_multi_table");
                }
                found
            }
            Val::Const(c) => BTreeSet::from([c]),
            // (a table address made of a load or a sum: never seen)
            _ => return None,
        };
        // (no compare bounds the index's own register in the window: what the index is made of may, as `and w12, w24,
        // #1; orr w12, w12, w17` with w17 a one-bit field, made in the load's own block. With a general bound too, the
        // larger: a compare of another register in the window, as a range check before a masked switch, would cut the
        // table short, and a case past it would go to the switch's default, an indirect jump to no function; a bound
        // too large reads at most into what follows the table, then until an entry leaves the function)
        let mut made = || {
            let (lo, hi) = self.function_extent(load_pc);
            if hi - lo > MAX_EXTENT {
                return None;
            }
            let preds = self.preds(lo, hi);
            // (the BRs whose targets are not known yet, this one among them: the only edges `preds` lacks)
            let open: Vec<u64> = (lo..hi)
                .step_by(4)
                .filter(|&pc| {
                    self.image.in_code(pc)
                        && matches!(Kind::of(self.word(pc), pc), Kind::Br(_))
                        && !self.jt.contains_key(&pc)
                })
                .collect();
            self.index_bound(&Flow { preds: &preds, start: lo, open: &open }, load_pc, *index, &mut Vec::new())
        };
        let count = own.or_else(|| match (general, made().filter(|&c| c > 0)) {
            (Some(g), Some(m)) => Some(g.max(m)),
            (g, m) => g.or(m),
        });
        let count = count.filter(|&c| 0 < c && c <= 4096);
        let read = |table: u64, limit: u64, strict: bool| -> Option<Vec<u64>> {
            let mut out = Vec::new();
            for i in 0..limit {
                let Some(bytes) = self.image.read(table.wrapping_add(i * u64::from(size)), size as usize) else {
                    break;
                };
                let entry = entry_value(bytes, signed);
                let t = base.wrapping_add(entry.checked_shl(k).unwrap_or(0));
                if !(lo <= t && t < hi && self.image.in_code(t)) {
                    if strict {
                        return None;
                    }
                    break;
                }
                out.push(t);
            }
            Some(out)
        };
        let mut targets: Vec<u64> = Vec::new();
        let mut bounded = true;
        let mut out_of_function = 0;
        for &table in &tables {
            let got = match count.and_then(|c| read(table, c, true)) {
                Some(got) => got,
                None => {
                    // no bound, or one that over-reads into the next table: read until an entry leaves the function
                    if count.is_some() {
                        out_of_function += 1;
                    }
                    bounded = false;
                    read(table, if size == 1 { 256 } else { 1024 }, false).unwrap_or_default()
                }
            };
            // (a table's targets that an earlier table had are left out; repeats within one table stay)
            let before: HashSet<u64> = targets.iter().copied().collect();
            targets.extend(got.into_iter().filter(|t| !before.contains(t)));
        }
        for _ in 0..out_of_function {
            self.stats.bump("jt_out_of_function");
        }
        if targets.is_empty() {
            return None;
        }
        self.stats.bump(if bounded && count.is_some() { "jt_bounded" } else { "jt_unbounded" });
        Some(targets)
    }

    /// The predecessors of the code in [lo, hi): fall-through, direct branches, and the jump tables found so far.
    /// Made once per extent, then kept current by `add_jt_edges`.
    fn preds(&mut self, lo: u64, hi: u64) -> Rc<Preds> {
        if let Some(p) = self.preds.get(&lo) {
            return Rc::clone(p);
        }
        let mut p: Preds = HashMap::new();
        for pc in (lo..hi).step_by(4) {
            if !self.image.in_code(pc) {
                continue;
            }
            let succ: Vec<u64> = match Kind::of(self.word(pc), pc) {
                Kind::B(t) => vec![t],
                Kind::BCond(t) | Kind::Cbz(t) | Kind::Tbz(t) => vec![t, pc + 4],
                Kind::Br(_) => self.jt.get(&pc).cloned().unwrap_or_default(),
                Kind::Ret | Kind::Trap => Vec::new(),
                _ => vec![pc + 4],
            };
            for s in succ {
                p.entry(s).or_default().push(pc);
            }
        }
        let p = Rc::new(p);
        self.preds.insert(lo, Rc::clone(&p));
        p
    }

    /// Every ADRP (+ ADD) constant reaching `at` in register `reg` along the function's code, sorted.
    fn reaching_constants(&mut self, at: u64, reg: u32) -> Vec<u64> {
        self.reaching(at, Loc::Reg(reg), 50_000, 0).into_iter().collect()
    }

    /// Walk back from `at` until each path writes `loc`. A value copied (MOV), or spilled to a
    /// stack slot and reloaded, is followed to where it was set; a call clobbering a caller-saved register, or any
    /// other write, ends its path with nothing.
    fn reaching(&mut self, at: u64, loc: Loc, limit: usize, depth: u32) -> BTreeSet<u64> {
        let (lo, hi) = self.function_extent(at);
        let mut found = BTreeSet::new();
        if hi - lo > MAX_EXTENT || depth > 4 {
            return found;
        }
        let preds = self.preds(lo, hi);
        let mut seen: HashSet<u64> = HashSet::new();
        let mut todo = vec![at];
        while !todo.is_empty() && seen.len() < limit {
            let next = todo.pop().expect("not empty");
            let Some(ps) = preds.get(&next) else { continue };
            for &p in ps {
                if !seen.insert(p) {
                    continue;
                }
                let w = self.word(p);
                match loc {
                    Loc::Reg(x) => {
                        if matches!(Kind::of(w, p), Kind::Bl(_) | Kind::Blr(_)) && (x <= 18 || x == 30) {
                            continue;
                        }
                        if !writes(w, x) {
                            todo.push(p);
                            continue;
                        }
                        if let Some(page) = adrp(w, p) {
                            found.insert(page);
                        } else if let Some(imm) = add_imm(w) {
                            if let Some(v) = self.hoisted_constant(p, (w >> 5) & 31, 64) {
                                found.insert(v.wrapping_add(imm));
                            }
                        } else if w & 0xffe0_ffe0 == 0xaa00_03e0 {
                            found.extend(self.reaching(p, Loc::Reg((w >> 16) & 31), limit, depth + 1));
                        } else if w & 0xffc0_03e0 == 0xf940_03e0 {
                            let slot = i64::from((w >> 10) & 0xfff) * 8;
                            found.extend(self.reaching(p, Loc::Slot(slot), limit, depth + 1));
                        } else if w & 0xffc0_03e0 == 0xa940_03e0 {
                            let off = pair_offset(w).unwrap_or(0) + if w & 31 != x { 8 } else { 0 };
                            found.extend(self.reaching(p, Loc::Slot(off), limit, depth + 1));
                        }
                    }
                    Loc::Slot(x) => {
                        if w & 0xffc0_03e0 == 0xf900_03e0 && i64::from((w >> 10) & 0xfff) * 8 == x {
                            found.extend(self.reaching(p, Loc::Reg(w & 31), limit, depth + 1));
                            continue;
                        }
                        if w & 0xffc0_03e0 == 0xa900_03e0 {
                            let off = pair_offset(w).unwrap_or(0);
                            if x == off || x == off + 8 {
                                let r = if x == off { w & 31 } else { (w >> 10) & 31 };
                                found.extend(self.reaching(p, Loc::Reg(r), limit, depth + 1));
                                continue;
                            }
                        }
                        let sp_moved = (w & 31 == 31 && matches!(w & 0x7f80_0000, 0x5100_0000 | 0x1100_0000))
                            || ((w >> 5) & 31 == 31
                                && (matches!(w & 0x3b20_0c00, 0x3800_0c00 | 0x3800_0400)
                                    || w & 0x3a80_0000 == 0x2880_0000));
                        if sp_moved {
                            continue; // SP moved: the slots differ
                        }
                        todo.push(p);
                    }
                }
            }
        }
        found
    }

    /// A power of two the value in `reg` is below at `before` (the table's load), from what wrote it on every path
    /// that reaches `before` in the function (its predecessors: fall-through, branches, the jump tables found so far):
    /// an AND with a low mask, an unsigned bitfield extract or insert, an ORR of two such values, a move of one. Any
    /// path that reaches the function's start, crosses a call (a caller-saved register) or ends at another write
    /// bounds nothing, as does an entry (its callers' values). Other code no known edge reaches is reached from the
    /// function's open BRs (`flow.open`).
    /// (`active`: the searches this one is inside; one of them again is a copy going round a loop, which adds no
    /// value of its own: 0)
    fn index_bound(&self, flow: &Flow, before: u64, reg: u32, active: &mut Vec<(u64, u32)>) -> Option<u64> {
        if active.contains(&(before, reg)) {
            return Some(0);
        }
        if active.len() >= 8 {
            return None;
        }
        active.push((before, reg));
        let bound = self.index_bound_paths(flow, before, reg, active);
        active.pop();
        bound
    }

    fn index_bound_paths(&self, flow: &Flow, before: u64, reg: u32, active: &mut Vec<(u64, u32)>) -> Option<u64> {
        let mut bound = 0;
        let mut seen: HashSet<u64> = HashSet::new();
        let mut todo = vec![before];
        while let Some(at) = todo.pop() {
            // (an entry is reached from its callers too, with any value)
            if self.entries.contains(&at) {
                return None;
            }
            let ps = match flow.preds.get(&at).filter(|ps| !ps.is_empty()) {
                Some(ps) => ps.as_slice(),
                None if at != flow.start && !flow.open.is_empty() => flow.open,
                None => return None,
            };
            for &pc in ps {
                if !seen.insert(pc) {
                    continue;
                }
                if seen.len() > 512 || !self.image.in_code(pc) {
                    return None;
                }
                let w = self.word(pc);
                if reg <= 18 && matches!(Kind::of(w, pc), Kind::Bl(_) | Kind::Blr(_)) {
                    return None;
                }
                if writes(w, reg) {
                    bound = bound.max(self.bound_of(flow, pc, w, active)?);
                } else {
                    todo.push(pc);
                }
            }
        }
        Some(bound)
    }

    /// The power of two the value `w` (at `pc`) writes is below, if it says one ([`Self::index_bound`])
    fn bound_of(&self, flow: &Flow, pc: u64, w: u32, active: &mut Vec<(u64, u32)>) -> Option<u64> {
        use a64::{BitfieldOp, Insn, LogicOp, Shift};
        let bits = |n: u32| (n <= 12).then(|| 1u64 << n);
        match a64::decode(w)? {
            Insn::LogicalImm { op: LogicOp::And | LogicOp::Ands, imm, .. }
                if imm < 4096 && (imm + 1).is_power_of_two() =>
            {
                Some(imm + 1)
            }
            Insn::Bitfield { op: BitfieldOp::Ubfm, sf, immr, imms, .. } => {
                let (immr, imms, size) = (u32::from(immr), u32::from(imms), if sf { 64 } else { 32 });
                bits(if imms >= immr { imms - immr + 1 } else { size - immr + imms + 1 })
            }
            Insn::LogicalReg { op: LogicOp::Orr, rn, rm, shift: Shift::Lsl, amount: 0, .. } => {
                let rm = self.index_bound(flow, pc, u32::from(rm.0), active)?;
                if rn.0 == 31 { Some(rm) } else { Some(rm.max(self.index_bound(flow, pc, u32::from(rn.0), active)?)) }
            }
            _ => None,
        }
    }

    /// The constant ADRP (+ ADD) last wrote to `reg` before `before`, scanning back in address order (which stands
    /// for "earlier", as LLVM lays out a loop-invariant table address).
    fn hoisted_constant(&self, before: u64, reg: u32, scan: u32) -> Option<u64> {
        let mut pc = before.checked_sub(4)?;
        for _ in 0..scan {
            if !self.image.in_code(pc) {
                return None;
            }
            let w = self.word(pc);
            if writes(w, reg) {
                if let Some(page) = adrp(w, pc) {
                    return Some(page);
                }
                // (a register move: the constant the source holds there)
                if w & 0xffe0_ffe0 == 0xaa00_03e0 {
                    return self.hoisted_constant(pc, (w >> 16) & 31, scan);
                }
                if let Some(imm) = add_imm(w) {
                    return self.hoisted_constant(pc, (w >> 5) & 31, 64).map(|v| v.wrapping_add(imm));
                }
                return None;
            }
            pc = pc.checked_sub(4)?;
        }
        None
    }

    /// The one ADRP(+ADD) constant the function ever assigns to `reg`, else the one
    /// ADRP(+ADD) or ADR constant: the first reads every word of the extent, the second only its code, so each keeps
    /// its own ADRP pages.
    fn function_constant(&mut self, br_pc: u64, reg: u32) -> Option<u64> {
        let (lo, hi) = self.function_extent(br_pc);
        if hi - lo > MAX_EXTENT {
            return None;
        }
        let (mut all_pages, mut code_pages) = (HashMap::new(), HashMap::new());
        let (mut adds, mut code_refs) = (BTreeSet::new(), BTreeSet::new());
        for pc in (lo..hi).step_by(4) {
            let Some(w) = self.image.word(pc) else { continue };
            let in_code = self.image.in_code(pc);
            let d = w & 31;
            if let Some(page) = adrp(w, pc) {
                all_pages.insert(d, page);
                if in_code {
                    code_pages.insert(d, page);
                }
            } else if let Some(imm) = add_imm(w) {
                let n = (w >> 5) & 31;
                if d == reg {
                    adds.extend(all_pages.get(&n).map(|p: &u64| p.wrapping_add(imm)));
                    if in_code {
                        code_refs.extend(code_pages.get(&n).map(|p: &u64| p.wrapping_add(imm)));
                    }
                }
            } else if let Some(t) = adr(w, pc).filter(|_| d == reg && in_code) {
                code_refs.insert(t);
            }
        }
        if adds.len() == 1 {
            return adds.pop_first();
        }
        if code_refs.len() == 1 {
            self.stats.bump("jt_adr_table");
            return code_refs.pop_first();
        }
        None
    }

    /// Every `adr xreg, label` into the same function: the candidates of a BR whose value the window could not pin
    /// down (the switch keeps a default case for any it misses).
    fn adr_targets(&mut self, br_pc: u64, reg: u32) -> Option<Vec<u64>> {
        let (lo, hi) = self.function_extent(br_pc);
        if hi - lo > MAX_EXTENT {
            return None;
        }
        let mut targets = Vec::new();
        for pc in (lo..hi).step_by(4) {
            let Some(w) = self.image.word(pc) else { continue };
            if let Some(t) = adr(w, pc).filter(|&t| w & 31 == reg && lo <= t && t < hi && self.image.in_code(t)) {
                targets.push(t);
            }
        }
        if targets.is_empty() {
            return None;
        }
        self.stats.bump("br_adr_candidates");
        Some(targets)
    }
}

/// A table entry's value, sign-extended when `signed`, as a 64-bit word.
fn entry_value(bytes: &[u8], signed: bool) -> u64 {
    let mut raw = [0u8; 8];
    raw[..bytes.len()].copy_from_slice(bytes);
    let v = u64::from_le_bytes(raw);
    let bits = 8 * bytes.len() as u32;
    match (signed, bits) {
        (true, 8) => v as u8 as i8 as u64,
        (true, 16) => v as u16 as i16 as u64,
        (true, 32) => v as u32 as i32 as u64,
        _ => v,
    }
}

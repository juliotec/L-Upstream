//! The discovery itself: every entry walked in address order, the jump tables it missed tried again once every function's are known, the
//! PLT stubs, and the setjmp owners, whose calls' return addresses become entries of their own.
//!
//! A function is an entry and every instruction reachable from it without crossing another entry. The walks depend on
//! each other in one way: a jump table found by the predecessors' search ([`Discovery::reaching_constants`]) sees the
//! tables found so far in its function's unwind range. So the order is fixed: entries in address order, each
//! function's instructions in its walk's order.

use crate::classify::{Kind, adrp};
use image::Image;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// The symbols whose callers own a setjmp.
const SETJMP_NAMES: [&str; 3] = ["__setjmp", "setjmp", "_setjmp"];

/// What a discovery starts from: the entries, the host's functions, the unwind starts and the symbol names.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// Every entry, the hooks' addresses included.
    pub entries: BTreeSet<u64>,
    /// The host's functions at guest addresses (HLE and stubs), by C name: not walked.
    pub hooks: BTreeMap<u64, String>,
    /// The `.eh_frame_hdr` function starts, sorted (repeats kept): a jump table's targets stay inside one range.
    pub eh_starts: Vec<u64>,
    /// The defined function symbols' names, by address (the first in module and table order).
    pub names: HashMap<u64, String>,
}

/// One walked function: its instructions (as runs), its labels, and its BLs in walk order.
#[derive(Clone, Debug)]
pub(crate) struct Walked {
    pub(crate) function: crate::plan::Function,
    /// (pc, target) of each BL, in the order the walk met them.
    pub(crate) calls: Vec<(u64, u64)>,
}

/// Counters of what the search found and missed (what `longshot-builder dev discover` prints; nothing depends on them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Counts by name (jt_bounded, jt_no_table ...).
    pub counts: BTreeMap<&'static str, u64>,
}

impl Stats {
    pub(crate) fn bump(&mut self, name: &'static str) {
        *self.counts.entry(name).or_default() += 1;
    }
}

/// A function extent's predecessors: for each address, the instructions that may go to it.
pub(crate) type Preds = HashMap<u64, Vec<u64>>;

/// A discovery in progress, then done.
pub struct Discovery<'a> {
    pub(crate) image: &'a Image,
    pub(crate) entries: HashSet<u64>,
    entry_order: BTreeSet<u64>,
    hooks: BTreeMap<u64, String>,
    pub(crate) eh_starts: Vec<u64>,
    names: HashMap<u64, String>,
    funcs: BTreeMap<u64, Walked>,
    pub(crate) jt: HashMap<u64, Vec<u64>>,
    /// The BRs whose table was not found: (pc, register), in the order first missed, with their places.
    pub(crate) jt_missing: Vec<(u64, u32)>,
    pub(crate) jt_missing_at: HashMap<u64, usize>,
    pub(crate) preds: HashMap<u64, Rc<Preds>>,
    plt: BTreeMap<u64, crate::plan::Plt>,
    setjmp_owners: BTreeMap<u64, Vec<u64>>,
    /// The counters.
    pub stats: Stats,
}

impl<'a> Discovery<'a> {
    /// A discovery of `image` from `inputs`, not run yet.
    pub fn new(image: &'a Image, inputs: Inputs) -> Discovery<'a> {
        let eh_starts =
            if inputs.eh_starts.is_empty() { inputs.entries.iter().copied().collect() } else { inputs.eh_starts };
        Discovery {
            image,
            entries: inputs.entries.iter().copied().collect(),
            entry_order: inputs.entries,
            hooks: inputs.hooks,
            eh_starts,
            names: inputs.names,
            funcs: BTreeMap::new(),
            jt: HashMap::new(),
            jt_missing: Vec::new(),
            jt_missing_at: HashMap::new(),
            preds: HashMap::new(),
            plt: BTreeMap::new(),
            setjmp_owners: BTreeMap::new(),
            stats: Stats::default(),
        }
    }

    /// The discovery: every entry walked, the missed tables tried again, the PLT stubs and the setjmp owners.
    pub fn run(&mut self) {
        let order: Vec<u64> = self.entry_order.iter().copied().filter(|e| !self.hooks.contains_key(e)).collect();
        for e in order {
            let w = self.walk(e);
            self.funcs.insert(e, w);
        }
        // A table found through another switch's edges needs those first: the misses tried once more with every
        // function's tables known (the predecessors made again, and not kept current here), and the
        // functions that hold a newly found one walked again.
        let missing = std::mem::take(&mut self.jt_missing);
        self.jt_missing_at.clear();
        self.preds.clear();
        let mut redo = BTreeSet::new();
        for &(pc, reg) in &missing {
            if let Some(targets) = self.jump_table(pc, reg) {
                self.jt.insert(pc, targets);
                redo.extend(self.funcs.iter().filter(|(_, w)| w.function.contains(pc)).map(|(e, _)| *e));
            }
        }
        for e in redo {
            let w = self.walk(e);
            self.funcs.insert(e, w);
        }
        let second = missing.len() as u64 - self.jt_missing.len() as u64;
        self.stats.counts.insert("jt_second_pass", second);
        self.stats.counts.insert("jt_no_table", self.jt_missing.len() as u64);
        let stubs: Vec<(u64, crate::plan::Plt)> =
            self.funcs.keys().filter_map(|&e| Some((e, self.plt_target(e)?))).collect();
        self.plt.extend(stubs);
        self.find_setjmp_owners();
    }

    /// The function at `entry`: its walk.
    pub(crate) fn walk(&mut self, entry: u64) -> Walked {
        let mut body: HashSet<u64> = HashSet::new();
        let mut order: Vec<u64> = Vec::new();
        let mut labels = BTreeSet::new();
        let mut calls = Vec::new();
        let mut todo = vec![entry];
        while let Some(mut pc) = todo.pop() {
            while !body.contains(&pc) {
                if (pc != entry && self.entries.contains(&pc)) || !self.image.in_code(pc) {
                    break;
                }
                let w = self.image.word(pc).unwrap_or(0);
                let kind = Kind::of(w, pc);
                body.insert(pc);
                order.push(pc);
                match kind {
                    Kind::B(t) => {
                        self.visit(entry, t, &mut labels, &mut todo);
                        break;
                    }
                    Kind::BCond(t) | Kind::Cbz(t) | Kind::Tbz(t) => self.visit(entry, t, &mut labels, &mut todo),
                    Kind::Br(reg) => {
                        if let Some(targets) = self.jump_table(pc, u32::from(reg)) {
                            self.add_jt_edges(pc, &targets);
                            for &t in &targets {
                                self.visit(entry, t, &mut labels, &mut todo);
                            }
                            self.jt.insert(pc, targets);
                        }
                        break;
                    }
                    Kind::Ret | Kind::Trap => break,
                    Kind::Bl(t) => calls.push((pc, t)),
                    _ => {}
                }
                pc += 4;
            }
        }
        Walked { function: crate::plan::Function::from_pcs(order, labels), calls }
    }

    /// A branch of the function at `entry` to `t`: a label to walk from, unless `t` is another entry or no code.
    fn visit(&self, entry: u64, t: u64, labels: &mut BTreeSet<u64>, todo: &mut Vec<u64>) {
        if t != entry && self.entries.contains(&t) {
            return;
        }
        if self.image.in_code(t) {
            labels.insert(t);
            todo.push(t);
        }
    }

    /// Keep a predecessor map current when a jump table of its extent is found.
    fn add_jt_edges(&mut self, br_pc: u64, targets: &[u64]) {
        let (lo, _) = self.function_extent(br_pc);
        if let Some(preds) = self.preds.get_mut(&lo) {
            let preds = Rc::make_mut(preds);
            for &t in targets {
                preds.entry(t).or_default().push(br_pc);
            }
        }
    }

    /// The unwind range `pc` is in: [the start at or below it, the next start). Below the
    /// first start it begins at 0; past the last it has no end.
    pub(crate) fn function_extent(&self, pc: u64) -> (u64, u64) {
        let i = self.eh_starts.partition_point(|&s| s <= pc);
        let lo = if i > 0 { self.eh_starts[i - 1] } else { 0 };
        let hi = self.eh_starts.get(i).copied().unwrap_or(u64::MAX);
        (lo, hi)
    }

    /// A PLT stub at `e` (adrp x16 / ldr x17, [x16, #off] / add x16 / br x17): its target, read from the relocated
    /// GOT, when that target is an entry.
    fn plt_target(&self, e: u64) -> Option<crate::plan::Plt> {
        let w = |k: u64| self.image.word(e + 4 * k);
        let (w0, w1, w2, w3) = (w(0)?, w(1)?, w(2)?, w(3)?);
        if w0 & 0x9f00_001f != 0x9000_0010
            || w1 & 0xffc0_03ff != 0xf940_0211
            || w2 & 0xffc0_03ff != 0x9100_0210
            || w3 != 0xd61f_0220
        {
            return None;
        }
        let slot = adrp(w0, e)?.wrapping_add(u64::from((w1 >> 10) & 0xfff) * 8);
        let target = self.image.u64(slot)?;
        self.entries.contains(&target).then_some(crate::plan::Plt { target, slot })
    }

    /// `t`, past up to four PLT stubs.
    fn callee(&self, mut t: u64) -> u64 {
        for _ in 0..4 {
            match self.plt.get(&t) {
                Some(stub) => t = stub.target,
                None => break,
            }
        }
        t
    }

    /// The functions that call `__setjmp`, with their calls' return addresses; each return address an entry, walked.
    fn find_setjmp_owners(&mut self) {
        let mut owners: Vec<(u64, Vec<u64>)> = Vec::new();
        for (&e, w) in &self.funcs {
            let rets: Vec<u64> = w
                .calls
                .iter()
                .filter(|(_, t)| self.names.get(&self.callee(*t)).is_some_and(|n| SETJMP_NAMES.contains(&n.as_str())))
                .map(|(pc, _)| pc + 4)
                .collect();
            if !rets.is_empty() {
                owners.push((e, rets));
            }
        }
        for (_, rets) in &owners {
            for &r in rets {
                if !self.funcs.contains_key(&r) {
                    self.entries.insert(r);
                    self.entry_order.insert(r);
                    let w = self.walk(r);
                    self.funcs.insert(r, w);
                }
            }
        }
        self.stats.counts.insert("setjmp_owners", owners.len() as u64);
        self.setjmp_owners = owners.into_iter().collect();
    }

    /// What the discovery found, as a plan (its image and code ranges from `image_path`, the image's).
    pub fn plan(&self, image_path: &std::path::Path) -> crate::plan::Plan {
        crate::plan::Plan {
            image: image_path.to_path_buf(),
            base: self.image.base(),
            code: self.image.code().to_vec(),
            entries: self.entry_order.clone(),
            hooks: self.hooks.clone(),
            functions: self.funcs.iter().map(|(e, w)| (*e, w.function.clone())).collect(),
            jump_tables: self.jt.iter().map(|(k, v)| (*k, v.clone())).collect(),
            plt: self.plt.clone(),
            setjmp_owners: self.setjmp_owners.clone(),
            ..crate::plan::Plan::default()
        }
    }
}

//! The lift plan: what the discovery ([`crate::discover`]) decided about the game's code, which the lift builds from.
//!
//! A function is an entry and every instruction reachable from it without crossing another entry (a64-kit's model):
//! its instructions are kept as runs of consecutive addresses, its labels are the branch targets inside it.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::PathBuf;

/// One function: its instructions and its labels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Function {
    /// Its instructions, as runs of consecutive addresses, in address order.
    pub runs: Vec<Range<u64>>,
    /// The addresses branches inside it go to.
    pub labels: BTreeSet<u64>,
}

impl Function {
    /// Its instructions' addresses, in order.
    pub fn pcs(&self) -> impl Iterator<Item = u64> + '_ {
        self.runs.iter().flat_map(|r| r.clone().step_by(4))
    }

    /// How many instructions it has.
    pub fn len(&self) -> usize {
        self.runs.iter().map(|r| ((r.end - r.start) / 4) as usize).sum()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Whether the instruction at `pc` is one of its own.
    pub fn contains(&self, pc: u64) -> bool {
        let i = self.runs.partition_point(|r| r.end <= pc);
        self.runs.get(i).is_some_and(|r| r.contains(&pc))
    }

    /// A function of these instructions (any order, no repeats needed) and labels.
    pub fn from_pcs(pcs: impl IntoIterator<Item = u64>, labels: BTreeSet<u64>) -> Function {
        let mut sorted: Vec<u64> = pcs.into_iter().collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut runs: Vec<Range<u64>> = Vec::new();
        for pc in sorted {
            match runs.last_mut() {
                Some(r) if r.end == pc => r.end = pc + 4,
                _ => runs.push(pc..pc + 4),
            }
        }
        Function { runs, labels }
    }
}

/// A PLT stub: the function it jumps to, and the GOT slot it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plt {
    /// The function the stub's GOT slot holds.
    pub target: u64,
    /// The slot's address.
    pub slot: u64,
}

/// A whole plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// The image's memory file.
    pub image: PathBuf,
    /// The image's base address.
    pub base: u64,
    /// The code ranges.
    pub code: Vec<Range<u64>>,
    /// Every entry: an address control may come in at, which starts a function of its own.
    pub entries: BTreeSet<u64>,
    /// The host's functions at guest addresses, by C name.
    pub hooks: BTreeMap<u64, String>,
    /// The functions, by entry.
    pub functions: BTreeMap<u64, Function>,
    /// The recovered jump tables: a BR's address and the targets it may go to.
    pub jump_tables: BTreeMap<u64, Vec<u64>>,
    /// The PLT stubs, by entry.
    pub plt: BTreeMap<u64, Plt>,
    /// The functions that call `__setjmp`, with their calls' return addresses.
    pub setjmp_owners: BTreeMap<u64, Vec<u64>>,
    /// BLRs called directly when they go to their usual target.
    pub devirt: BTreeMap<u64, u64>,
}

impl Plan {
    /// A call's target followed through PLT stubs (at most four deep) to the function it calls.
    pub fn callee(&self, mut t: u64) -> u64 {
        for _ in 0..4 {
            match self.plt.get(&t) {
                Some(stub) => t = stub.target,
                None => break,
            }
        }
        t
    }

    /// The functions the instruction at `pc` is lifted in (its own, and any whose body runs into it), by entry.
    pub fn holders(&self, pc: u64) -> Vec<u64> {
        self.functions.iter().filter(|(_, f)| f.contains(pc)).map(|(&e, _)| e).collect()
    }

    /// The function `pc` belongs to whose entry is at or below it, if `pc` is one of its instructions.
    pub fn function_at(&self, pc: u64) -> Option<(u64, &Function)> {
        let (&e, f) = self.functions.range(..=pc).next_back()?;
        f.contains(pc).then_some((e, f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_pcs_makes_runs() {
        let f = Function::from_pcs([8, 0, 4, 16, 4], BTreeSet::new());
        assert_eq!(f.runs, vec![0..12, 16..20]);
    }

    #[test]
    fn a_pc_belongs_to_the_function_that_holds_it() {
        let mut p = Plan::default();
        p.functions.insert(0x8000_0000, Function::from_pcs([0x8000_0000, 0x8000_0004, 0x8000_0020], BTreeSet::new()));
        assert_eq!(p.function_at(0x8000_0004).map(|(e, _)| e), Some(0x8000_0000));
        assert_eq!(p.function_at(0x8000_0010), None, "between its runs");
        assert_eq!(p.holders(0x8000_0020), vec![0x8000_0000]);
    }
}

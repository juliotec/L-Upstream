//! What the resolver reads of the game: a module's code and image (where signatures, strings and calls are searched),
//! and the lift's functions (the discovery's [`cfg::plan::Plan`]) over the image's words.

use cfg::plan::Plan;
use image::Image;
use std::collections::{BTreeMap, BTreeSet};

/// An instruction's kind as the lifted code treats it, which is what the site forms check: the decoder's
/// ([`a64::Kind`]), as discovery's.
pub use a64::Kind as InsnKind;

/// The lift's functions over the image: what locate() and place() read of the lifter.
#[derive(Clone, Copy)]
pub struct Lift<'a> {
    image: &'a Image,
    plan: &'a Plan,
}

/// One function of the lift.
#[derive(Clone, Copy)]
pub struct Function<'a> {
    /// Its entry.
    pub entry: u64,
    func: &'a cfg::plan::Function,
    image: &'a Image,
}

impl<'a> Function<'a> {
    /// Whether the instruction at `pc` is one of its own.
    pub fn contains(&self, pc: u64) -> bool {
        self.func.contains(pc)
    }

    /// Its instructions' addresses, in order.
    pub fn pcs(&self) -> impl Iterator<Item = u64> + 'a {
        self.func.pcs()
    }

    /// How many instructions it has.
    pub fn len(&self) -> usize {
        self.func.len()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.func.is_empty()
    }

    /// The word at `pc`, when `pc` is one of its instructions.
    pub fn word(&self, pc: u64) -> Option<u32> {
        if self.contains(pc) { self.image.word(pc) } else { None }
    }

    /// The kind of its instruction at `pc`.
    pub fn kind(&self, pc: u64) -> Option<InsnKind> {
        self.word(pc).map(|w| InsnKind::of(w, pc))
    }

    /// Its labels: the branch targets inside it.
    pub fn labels(&self) -> &'a BTreeSet<u64> {
        &self.func.labels
    }

    /// (pc, word) of each of its instructions, in order.
    pub fn words(&self) -> Vec<(u64, u32)> {
        self.pcs().filter_map(|pc| self.image.word(pc).map(|w| (pc, w))).collect()
    }
}

impl<'a> Lift<'a> {
    /// The plan's functions over this image.
    pub fn new(image: &'a Image, plan: &'a Plan) -> Lift<'a> {
        Lift { image, plan }
    }

    /// The function at `entry`.
    pub fn function(&self, entry: u64) -> Option<Function<'a>> {
        self.plan.functions.get(&entry).map(|func| Function { entry, func, image: self.image })
    }

    /// The functions the instruction at `pc` is lifted in, by entry ([`Plan::holders`]).
    pub fn holders(&self, pc: u64) -> Vec<u64> {
        self.plan.holders(pc)
    }

    /// A call's target followed through PLT stubs to what it calls ([`Plan::callee`]).
    pub fn callee(&self, t: u64) -> u64 {
        self.plan.callee(t)
    }

    /// Whether control may enter at `addr` as a function of its own.
    pub fn is_entry(&self, addr: u64) -> bool {
        self.plan.entries.contains(&addr)
    }

    /// Whether the function at `addr` calls `__setjmp` (its callers take a host setjmp).
    pub fn is_setjmp_owner(&self, addr: u64) -> bool {
        self.plan.setjmp_owners.contains_key(&addr)
    }

    /// The jump table of the BR at `pc`, if the lift recovered one.
    pub fn jump_table(&self, pc: u64) -> &'a [u64] {
        self.plan.jump_tables.get(&pc).map_or(&[], Vec::as_slice)
    }

    /// The PLT stubs' entries.
    pub fn plt_stubs(&self) -> impl Iterator<Item = u64> + 'a {
        self.plan.plt.keys().copied()
    }
}

/// A module as the searches read it: its code as words, its image (where strings are), and the exported symbols.
#[derive(Clone, Debug)]
pub struct Space<'a> {
    /// The code's first address.
    pub base: u64,
    /// The code, as instruction words.
    pub code: Vec<u32>,
    /// The module's image.
    pub image: &'a [u8],
    /// The image's first address.
    pub image_base: u64,
    /// The exported functions' addresses, by symbol.
    pub exports: BTreeMap<String, u64>,
}

impl<'a> Space<'a> {
    /// The module `name` of the image (its `.text` as code), or `None` when the image has no such module.
    pub fn of(image: &'a Image, name: &str) -> Option<Space<'a>> {
        let m = image.module(name)?;
        let text = m.text()?;
        let bytes = image.read(text.start, usize::try_from(text.end - text.start).ok()?)?;
        let code = bytes.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)).collect();
        let module = image.read(m.base, usize::try_from(m.size).ok()?)?;
        Some(Space { base: text.start, code, image: module, image_base: m.base, exports: image.exports().clone() })
    }

    /// The word at `addr`, if it is in the code.
    pub fn word(&self, addr: u64) -> Option<u32> {
        if addr & 3 != 0 || addr < self.base {
            return None;
        }
        self.code.get(usize::try_from((addr - self.base) / 4).ok()?).copied()
    }

    /// The word index of `addr` in the code.
    pub fn index(&self, addr: u64) -> Option<usize> {
        (addr & 3 == 0 && addr >= self.base).then(|| usize::try_from((addr - self.base) / 4).ok()).flatten()
    }
}

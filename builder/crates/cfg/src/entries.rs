//! Where discovery starts: the entry candidates of every module (its code start, init and fini, unwind starts, exports,
//! relocated code addresses, BL targets, init array), the code addresses the code takes with ADR or ADRP+ADD outside
//! the unwind-covered functions, the host's functions (the image's hooks), the `.eh_frame_hdr` starts and the symbol
//! names.

use crate::classify::{adr, adrp, bl_target};
use crate::module::{self, ModuleInfo, RelocKind};
use crate::walk::Inputs;
use image::Image;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;

/// The modules' tables could not be read.
pub type Error = module::Error;

/// Every module's tables, in the image's module order.
pub fn modules(image: &Image) -> Result<Vec<ModuleInfo>, Error> {
    image.modules().iter().map(|m| module::read(image, m)).collect()
}

/// Everything a discovery of `image` starts from, made from the image alone.
pub fn inputs(image: &Image) -> Result<Inputs, Error> {
    let modules = modules(image)?;
    let mut entries = candidates(image, &modules);
    let spans = {
        let mut s: Vec<Range<u64>> = modules.iter().flat_map(|m| module::fde_spans(image, m)).collect();
        s.sort_by_key(|r| (r.start, r.end));
        s
    };
    entries.extend(code_address_refs(image, &spans).into_iter().filter(|&a| image.in_code(a)));
    let hooks = image.hooks().iter().map(|h| (h.addr, h.name.clone())).collect::<std::collections::BTreeMap<_, _>>();
    entries.extend(hooks.keys().copied());
    let mut eh_starts: Vec<u64> = modules.iter().flat_map(|m| m.eh_funcs.iter().map(|f| m.base + f)).collect();
    eh_starts.sort_unstable();
    Ok(Inputs { entries, hooks, eh_starts, names: names(&modules) })
}

/// The image builder's entry candidates (each module's code start, DT_INIT and DT_FINI, its unwind starts, its
/// exported functions, every RELATIVE and ABS64 value that lands in code, every BL target, its init array), the ones
/// in code.
pub fn candidates(image: &Image, modules: &[ModuleInfo]) -> BTreeSet<u64> {
    let mut e = BTreeSet::new();
    for m in modules {
        e.insert(m.text.start);
        e.extend(m.init_fini.iter().map(|o| m.base + o));
        e.extend(m.eh_funcs.iter().map(|f| m.base + f));
        e.extend(m.symbols.iter().filter(|s| s.defined && s.kind == 2 && s.value != 0).map(|s| m.base + s.value));
        for r in &m.relocs {
            if matches!(r.kind, RelocKind::Relative | RelocKind::Abs64)
                && let Some(v) = image.u64(m.base + r.offset)
                && image.in_code(v)
            {
                e.insert(v);
            }
        }
        // BL targets (not ADR / ADRP+ADD targets: most are jump-table bases, which would cut functions in two)
        for pc in m.text.clone().step_by(4) {
            if let Some(t) = image.word(pc).and_then(|w| bl_target(w, pc)) {
                e.insert(t);
            }
        }
        for slot in &m.init_array {
            if let Some(v) = image.u64(m.base + slot)
                && image.in_code(v)
            {
                e.insert(v);
            }
        }
    }
    e.retain(|&a| image.in_code(a));
    e
}

/// The defined function symbols by address: the first name of each, in module and table order (nx_image's
/// `symbol_names`).
pub fn names(modules: &[ModuleInfo]) -> HashMap<u64, String> {
    let mut names = HashMap::new();
    for m in modules {
        for s in &m.symbols {
            if s.is_function() && !s.name.is_empty() {
                names.entry(m.base + s.value).or_insert_with(|| s.name.clone());
            }
        }
    }
    names
}

/// The 4-aligned code addresses ADR or ADRP followed by an ADD on its register (within four instructions) form in the
/// `.text` segments, without the ones strictly inside a function the unwind spans cover: such functions are found from their unwind entries anyway, and a label inside one as an entry
/// would lift the rest of it again.
pub fn code_address_refs(image: &Image, spans: &[Range<u64>]) -> BTreeSet<u64> {
    let texts = image.code();
    let mut found = BTreeSet::new();
    for text in texts {
        let words: Vec<u32> = text.clone().step_by(4).map(|pc| image.word(pc).unwrap_or(0)).collect();
        let pc_of = |i: usize| text.start + 4 * i as u64;
        for (i, &w) in words.iter().enumerate() {
            let pc = pc_of(i);
            if let Some(t) = adr(w, pc) {
                found.insert(t);
            } else if let Some(page) = adrp(w, pc) {
                for y in words.iter().skip(i + 1).take(4) {
                    if y & 0xffc0_0000 == 0x9100_0000 && (y >> 5) & 31 == w & 31 {
                        found.insert(page.wrapping_add(u64::from((y >> 10) & 0xfff)));
                    }
                }
            }
        }
    }
    found.retain(|&t| t % 4 == 0 && texts.iter().any(|r| r.contains(&t)));
    if spans.is_empty() {
        return found;
    }
    // the latest end among the spans starting at or before each address (spans may nest or repeat)
    let starts: Vec<u64> = spans.iter().map(|s| s.start).collect();
    let start_set: std::collections::HashSet<u64> = starts.iter().copied().collect();
    let mut best = Vec::with_capacity(spans.len());
    let mut most = 0u64;
    for s in spans {
        most = most.max(s.end);
        best.push(most);
    }
    found.retain(|&t| {
        let k = starts.partition_point(|&s| s <= t);
        let within = k > 0 && t < best[k - 1] && !start_set.contains(&t);
        !within
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::single_range_in_vec_init)] // (one span each: a list of ranges, as fde_spans gives)
    fn spans_filter_inner_addresses() {
        // a made-up text of: adr x0, #8 (to 0x1008, inside the span 0x1000..0x1100) and adr x1, #0x200 (outside it)
        let mut mem = vec![0u8; 0x400];
        mem[0..4].copy_from_slice(&0x1000_0040u32.to_le_bytes());
        mem[4..8].copy_from_slice(&0x1000_1001u32.to_le_bytes());
        let module = image::Module {
            name: "m".into(),
            base: 0x1000,
            size: 0x400,
            build_id: String::new(),
            segments: vec![image::Segment { name: ".text".into(), addr: 0x1000, size: 0x400 }],
        };
        let img = Image::new(0x1000, mem, vec![module]).unwrap();
        let all = code_address_refs(&img, &[]);
        assert!(all.contains(&0x1008) && all.contains(&0x1204));
        let outside = code_address_refs(&img, &[0x1000..0x1100]);
        assert!(!outside.contains(&0x1008) && outside.contains(&0x1204));
        // a span's own start is kept
        assert!(code_address_refs(&img, &[0x1008..0x1100]).contains(&0x1008));
    }
}

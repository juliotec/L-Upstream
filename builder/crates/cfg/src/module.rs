//! What each module says of itself, read from the image: its MOD0 header, its
//! ELF dynamic section, and through them its symbols, relocations, init array and `.eh_frame_hdr` table.
//!
//! The image holds every module as loaded and relocated. None of what is read here is relocated (the dynamic section,
//! the symbol, relocation and unwind tables hold module offsets), so it reads the same as from the module's own file;
//! the relocated values, where a relocation wrote one, are read from the image at the relocation's place.

use image::{Image, Module};
use std::ops::Range;

/// What can go wrong reading a module's tables.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The module's MOD0 header is not where its first instructions say.
    #[error("{module}: no MOD0 header at +{offset:#x}")]
    NoMod0 {
        /// The module's name.
        module: String,
        /// Where its `.text + 4` points.
        offset: u64,
    },
    /// A table runs out of the image.
    #[error("{module}: {what} at +{offset:#x} is outside the image")]
    Outside {
        /// The module's name.
        module: String,
        /// The table.
        what: &'static str,
        /// Its offset in the module.
        offset: u64,
    },
    /// The module has no `.text`.
    #[error("{0}: no .text segment")]
    NoText(String),
}

/// A dynamic symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Its name (lossy UTF-8).
    pub name: String,
    /// STT_*: 2 is a function.
    pub kind: u8,
    /// Whether the module defines it (a section index other than 0).
    pub defined: bool,
    /// Its value: a module offset for a defined symbol.
    pub value: u64,
}

impl Symbol {
    /// A defined function.
    pub fn is_function(&self) -> bool {
        self.defined && self.kind == 2
    }
}

/// The relocation types discovery reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelocKind {
    /// R_AARCH64_RELATIVE (and every RELR entry).
    Relative,
    /// R_AARCH64_ABS64.
    Abs64,
    /// Any other type (GLOB_DAT, JUMP_SLOT, TLSDESC ...).
    Other(u32),
}

/// A relocation: where it writes (a module offset) and of what kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reloc {
    /// The module offset it writes.
    pub offset: u64,
    /// Its type.
    pub kind: RelocKind,
}

/// A module's tables, at its place in the image.
#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// Its name.
    pub name: String,
    /// Where it is loaded.
    pub base: u64,
    /// Its size in the image.
    pub size: u64,
    /// Its `.text`, as absolute addresses.
    pub text: Range<u64>,
    /// DT_INIT and DT_FINI, as module offsets, when it has them.
    pub init_fini: Vec<u64>,
    /// Its dynamic symbols, in table order.
    pub symbols: Vec<Symbol>,
    /// Its relocations: RELA, then JMPREL, then RELR.
    pub relocs: Vec<Reloc>,
    /// DT_INIT_ARRAY's slots, as module offsets.
    pub init_array: Vec<u64>,
    /// `.eh_frame_hdr`'s place, module offsets.
    pub eh_frame_hdr: Range<u64>,
    /// The function starts `.eh_frame_hdr`'s table lists, as module offsets, in table order.
    pub eh_funcs: Vec<u64>,
}

const DT_PLTRELSZ: i64 = 2;
const DT_STRTAB: i64 = 5;
const DT_SYMTAB: i64 = 6;
const DT_RELA: i64 = 7;
const DT_RELASZ: i64 = 8;
const DT_INIT: i64 = 12;
const DT_FINI: i64 = 13;
const DT_SYMENT: i64 = 11;
const DT_JMPREL: i64 = 23;
const DT_INIT_ARRAY: i64 = 25;
const DT_INIT_ARRAYSZ: i64 = 27;
const DT_RELRSZ: i64 = 35;
const DT_RELR: i64 = 36;

/// A module's bytes, read through the image at module offsets.
struct View<'a> {
    image: &'a Image,
    base: u64,
    name: &'a str,
}

impl View<'_> {
    fn bytes(&self, off: u64, len: usize, what: &'static str) -> Result<&[u8], Error> {
        self.image.read(self.base.wrapping_add(off), len).ok_or_else(|| Error::Outside {
            module: self.name.to_string(),
            what,
            offset: off,
        })
    }
    fn u8(&self, off: u64, what: &'static str) -> Result<u8, Error> {
        Ok(self.bytes(off, 1, what)?[0])
    }
    fn u16(&self, off: u64, what: &'static str) -> Result<u16, Error> {
        let b = self.bytes(off, 2, what)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&self, off: u64, what: &'static str) -> Result<u32, Error> {
        let b = self.bytes(off, 4, what)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&self, off: u64, what: &'static str) -> Result<i32, Error> {
        Ok(self.u32(off, what)? as i32)
    }
    fn u64(&self, off: u64, what: &'static str) -> Result<u64, Error> {
        let b = self.bytes(off, 8, what)?;
        Ok(u64::from_le_bytes(b.try_into().expect("8 bytes")))
    }
    fn cstr(&self, off: u64) -> String {
        let start = self.base.wrapping_add(off);
        let mut end = start;
        while self.image.read(end, 1).is_some_and(|b| b[0] != 0) {
            end += 1;
        }
        let bytes = self.image.read(start, (end - start) as usize).unwrap_or_default();
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// `module`'s tables, read from `image`.
pub fn read(image: &Image, module: &Module) -> Result<ModuleInfo, Error> {
    let v = View { image, base: module.base, name: &module.name };
    let text = module.text().ok_or_else(|| Error::NoText(module.name.clone()))?;
    let mod0 = u64::from(v.u32(4, "the MOD0 offset")?);
    if v.bytes(mod0, 4, "MOD0")? != b"MOD0" {
        return Err(Error::NoMod0 { module: module.name.clone(), offset: mod0 });
    }
    let rel = |k: u64| -> Result<u64, Error> { Ok(mod0.wrapping_add(v.i32(mod0 + 4 + 4 * k, "MOD0")? as i64 as u64)) };
    let (dynamic, eh_start, eh_end) = (rel(0)?, rel(3)?, rel(4)?);

    // The dynamic section: the first value of each tag counts.
    let mut dt: Vec<(i64, u64)> = Vec::new();
    let mut off = dynamic;
    loop {
        let tag = v.u64(off, "the dynamic section")? as i64;
        let val = v.u64(off + 8, "the dynamic section")?;
        off += 16;
        if tag == 0 {
            break;
        }
        dt.push((tag, val));
    }
    let d = |tag: i64| dt.iter().find(|(t, _)| *t == tag).map_or(0, |(_, val)| *val);
    let has = |tag: i64| dt.iter().any(|(t, _)| *t == tag);

    let (strtab, symtab) = (d(DT_STRTAB), d(DT_SYMTAB));
    let syment = match d(DT_SYMENT) {
        0 => 24,
        n => n,
    };
    let count = if strtab > symtab { (strtab - symtab) / syment } else { 0 };
    let mut symbols = Vec::with_capacity(count as usize);
    for i in 0..count {
        let at = symtab + i * syment;
        let name = v.u32(at, "the symbol table")?;
        symbols.push(Symbol {
            name: v.cstr(strtab + u64::from(name)),
            kind: v.u8(at + 4, "the symbol table")? & 0xf,
            defined: v.u16(at + 6, "the symbol table")? != 0,
            value: v.u64(at + 8, "the symbol table")?,
        });
    }

    let mut relocs = Vec::new();
    for (start, size) in [(d(DT_RELA), d(DT_RELASZ)), (d(DT_JMPREL), d(DT_PLTRELSZ))] {
        for at in (start..start + size).step_by(24) {
            let kind = match v.u64(at + 8, "a relocation table")? as u32 {
                257 => RelocKind::Abs64,
                1027 => RelocKind::Relative,
                other => RelocKind::Other(other),
            };
            relocs.push(Reloc { offset: v.u64(at, "a relocation table")?, kind });
        }
    }
    let (relr, relr_size) = (d(DT_RELR), d(DT_RELRSZ));
    if relr != 0 && relr_size != 0 {
        // (RELR) an even entry is an address; an odd one a bitmap of the 63 words from the cursor
        let mut cursor = 0u64;
        for at in (relr..relr + relr_size).step_by(8) {
            let e = v.u64(at, "the RELR table")?;
            if e & 1 == 0 {
                relocs.push(Reloc { offset: e, kind: RelocKind::Relative });
                cursor = e + 8;
            } else {
                let mut bits = e >> 1;
                let mut i = 0;
                while bits != 0 {
                    if bits & 1 != 0 {
                        relocs.push(Reloc { offset: cursor + 8 * i, kind: RelocKind::Relative });
                    }
                    bits >>= 1;
                    i += 1;
                }
                cursor += 63 * 8;
            }
        }
    }

    let array = d(DT_INIT_ARRAY);
    let init_array = (0..d(DT_INIT_ARRAYSZ) / 8).map(|i| array + 8 * i).collect();
    let init_fini = [DT_INIT, DT_FINI].into_iter().filter(|t| has(*t)).map(d).collect();
    let eh_funcs = eh_frame_hdr_table(&v, eh_start, eh_end)?;
    Ok(ModuleInfo {
        name: module.name.clone(),
        base: module.base,
        size: module.size,
        text,
        init_fini,
        symbols,
        relocs,
        init_array,
        eh_frame_hdr: eh_start..eh_end,
        eh_funcs,
    })
}

/// The function starts of `.eh_frame_hdr`'s binary-search table (datarel sdata4 entries), none
/// unless the header is version 1 with those encodings.
fn eh_frame_hdr_table(v: &View<'_>, s: u64, e: u64) -> Result<Vec<u64>, Error> {
    if e <= s {
        return Ok(Vec::new());
    }
    let head = v.bytes(s, 4, ".eh_frame_hdr")?;
    let (ver, ptr_enc, cnt_enc, tbl_enc) = (head[0], head[1], head[2], head[3]);
    if ver != 1 || tbl_enc != 0x3b || cnt_enc != 0x03 || !matches!(ptr_enc & 0x0f, 0x0b | 0x03) {
        return Ok(Vec::new());
    }
    let count = u64::from(v.u32(s + 8, ".eh_frame_hdr")?);
    (0..count).map(|i| Ok(s.wrapping_add(v.i32(s + 12 + 8 * i, ".eh_frame_hdr")? as i64 as u64))).collect()
}

/// The (start, end) of every function `.eh_frame_hdr` has an FDE for, absolute, sorted: the FDE's pc_begin must agree
/// with the table, or it is skipped. Its header checks are only the table's (version 1, the table and count
/// encodings).
pub fn fde_spans(image: &Image, info: &ModuleInfo) -> Vec<Range<u64>> {
    let v = View { image, base: info.base, name: &info.name };
    let (s, e) = (info.eh_frame_hdr.start, info.eh_frame_hdr.end);
    let mut out = Vec::new();
    if e <= s {
        return out;
    }
    let Ok(head) = v.bytes(s, 4, ".eh_frame_hdr") else { return out };
    if head[0] != 1 || head[3] != 0x3b || head[2] != 0x03 {
        return out;
    }
    let Ok(count) = v.u32(s + 8, ".eh_frame_hdr") else { return out };
    for i in 0..u64::from(count) {
        let (Ok(loc), Ok(fde)) = (v.i32(s + 12 + 8 * i, ""), v.i32(s + 16 + 8 * i, "")) else { break };
        let loc = s.wrapping_add(loc as i64 as u64);
        let fde = s.wrapping_add(fde as i64 as u64);
        if fde + 16 > info.size {
            continue;
        }
        let (Ok(length), Ok(cie), Ok(begin), Ok(range)) =
            (v.u32(fde, ""), v.u32(fde + 4, ""), v.i32(fde + 8, ""), v.u32(fde + 12, ""))
        else {
            continue;
        };
        if length < 12 || cie == 0 || fde.wrapping_add(8).wrapping_add(begin as i64 as u64) != loc {
            continue;
        }
        out.push(info.base + loc..info.base + loc + u64::from(range));
    }
    out
}

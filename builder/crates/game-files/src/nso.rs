//! The game's code modules (NSO, switchbrew "NSO"): loaded for the image, and unpacked in place.
//!
//! - A header of 0x100 bytes: three segments (.text, .rodata, .data), each a file offset, a memory offset and a size;
//!   each compressed when flags bit 0, 1 or 2 is set, with LZ4 blocks, or (bit 7, Switch firmware 22.0.0 on) with ZBIC
//!   ([`crate::zstd`]); bits 3 to 5 say the segment's SHA-256 (of its plain bytes, at 0xA0) is there to check. The bss
//!   size at 0x3C, the build id at 0x40.
//! - `.text + 4` holds the offset of the MOD0 header, which points (from itself) at the ELF dynamic section, the bss,
//!   `.eh_frame_hdr` and the module object.
//! - The dynamic section is AArch64 ELF's: the symbols, RELA and JMPREL relocations, and on every Switch 2 module DT_RELR
//!   (the packed form of the relative relocations: each entry an address, even, or a bitmap of the 63 words after the
//!   cursor, odd). A RELR relocation's addend is the word it relocates, read here as the module ships it (before any
//!   ExeFS patch: [`crate::patches`]).
//! - `.eh_frame_hdr`'s sorted table holds the start of every function with unwind information.

use build_core::{Error, IoContext, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

/// The modules an exefs may hold, in the order the system's loader takes them (rtld, main, subsdk0..9, sdk).
pub const MODULE_ORDER: [&str; 13] = [
    "rtld", "main", "subsdk0", "subsdk1", "subsdk2", "subsdk3", "subsdk4", "subsdk5", "subsdk6", "subsdk7", "subsdk8",
    "subsdk9", "sdk",
];

/// An NSO's flags bit for ZBIC segments.
pub const ZBIC_FLAG: u32 = 1 << 7;

const SEGMENT_NAMES: [&str; 3] = [".text", ".rodata", ".data"];

// ---- the dynamic section's tags read here
/// DT_STRTAB: the string table.
pub const DT_STRTAB: i64 = 5;
/// DT_SYMTAB: the symbol table.
pub const DT_SYMTAB: i64 = 6;
/// DT_RELA: the RELA relocations.
pub const DT_RELA: i64 = 7;
/// DT_RELASZ: their size.
pub const DT_RELASZ: i64 = 8;
/// DT_STRSZ: the string table's size.
pub const DT_STRSZ: i64 = 10;
/// DT_SYMENT: a symbol's size.
pub const DT_SYMENT: i64 = 11;
/// DT_INIT: the module's `_init`, which the system's loader calls.
pub const DT_INIT: i64 = 12;
/// DT_FINI: the module's `_fini`.
pub const DT_FINI: i64 = 13;
/// DT_PLTRELSZ: the JMPREL relocations' size.
pub const DT_PLTRELSZ: i64 = 2;
/// DT_JMPREL: the JMPREL (PLT) relocations.
pub const DT_JMPREL: i64 = 23;
/// DT_INIT_ARRAY: the init array.
pub const DT_INIT_ARRAY: i64 = 25;
/// DT_INIT_ARRAYSZ: its size.
pub const DT_INIT_ARRAYSZ: i64 = 27;
/// DT_RELRSZ: the RELR relocations' size.
pub const DT_RELRSZ: i64 = 35;
/// DT_RELR: the packed relative relocations.
pub const DT_RELR: i64 = 36;
/// DT_RELRENT: a RELR entry's size.
pub const DT_RELRENT: i64 = 37;

/// A segment of a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// `.text`, `.rodata` or `.data`.
    pub name: &'static str,
    /// Its offset in the module's image.
    pub offset: u64,
    /// Its size.
    pub size: u64,
}

/// What a relocation writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelocKind {
    /// The module's base plus the addend (R_AARCH64_RELATIVE, and every DT_RELR entry).
    Relative,
    /// A symbol's address plus the addend (R_AARCH64_ABS64).
    Abs64,
    /// The same, for a GOT entry (R_AARCH64_GLOB_DAT).
    GlobDat,
    /// The same, for a PLT slot (R_AARCH64_JUMP_SLOT).
    JumpSlot,
    /// Another type, by its number (the image refuses it).
    Other(u32),
}

impl RelocKind {
    fn of(t: u32) -> RelocKind {
        match t {
            257 => RelocKind::Abs64,
            1025 => RelocKind::GlobDat,
            1026 => RelocKind::JumpSlot,
            1027 => RelocKind::Relative,
            other => RelocKind::Other(other),
        }
    }
}

/// One relocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reloc {
    /// The word it writes, from the module's start.
    pub offset: u64,
    /// What it writes.
    pub kind: RelocKind,
    /// Its symbol's name (index into [`Nso::symbols`]), if it has one.
    pub symbol: Option<usize>,
    /// The addend.
    pub addend: i64,
}

/// A symbol of the module's dynamic symbol table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Its name (UTF-8, a byte that is not replaced by U+FFFD).
    pub name: String,
    /// STT_*: 0 none, 1 object, 2 function, ...
    pub kind: u8,
    /// STB_*: 0 local, 1 global, 2 weak.
    pub bind: u8,
    /// Whether the module defines it (a section index other than 0).
    pub defined: bool,
    /// Its value: an offset in the module when defined.
    pub value: u64,
    /// Its size.
    pub size: u64,
}

impl Symbol {
    /// STT_FUNC.
    pub fn is_function(&self) -> bool {
        self.kind == 2
    }

    /// STB_WEAK.
    pub fn is_weak(&self) -> bool {
        self.bind == 2
    }
}

/// A module loaded: its image (segments in place, bss zeroed) and what its tables say.
#[derive(Clone, Debug)]
pub struct Nso {
    /// Its file's name: rtld, main, subsdk0, sdk.
    pub name: String,
    /// The build id, hex, its trailing zero bytes left off.
    pub build_id: String,
    /// The header's bss size.
    pub bss_size: u64,
    /// The three segments, in the image.
    pub segments: [Segment; 3],
    /// The image: from offset 0 to the end of the bss.
    pub image: Vec<u8>,
    /// Where the MOD0 header is.
    pub mod0: u64,
    /// The bss, from the MOD0 header.
    pub bss: (u64, u64),
    /// The dynamic section: each tag's values in order.
    pub dynamic: BTreeMap<i64, Vec<u64>>,
    /// The dynamic symbols.
    pub symbols: Vec<Symbol>,
    /// RELA, then JMPREL, then DT_RELR's (as relative).
    pub relocs: Vec<Reloc>,
    /// The function starts of `.eh_frame_hdr`'s table.
    pub eh_funcs: Vec<u64>,
}

use crate::le::{i32_at, u32_at, u64_at};

/// The most a module's image (its segments and its bss) may take: the game's largest is about 100 MB. A header's
/// sizes are u32s, so a damaged or crafted one could otherwise ask for gigabytes before anything is checked.
pub const MAX_MODULE: usize = 1 << 30;

/// A segment's header fields: file offset, memory offset, size, compressed size.
fn segment_fields(raw: &[u8], i: usize) -> Option<(usize, usize, usize, usize)> {
    Some((
        u32_at(raw, 0x10 + 0x10 * i)? as usize,
        u32_at(raw, 0x14 + 0x10 * i)? as usize,
        u32_at(raw, 0x18 + 0x10 * i)? as usize,
        u32_at(raw, 0x60 + 4 * i)? as usize,
    ))
}

/// A segment's plain bytes: decompressed as its flags say, its hash checked when the header has one.
fn segment_data(name: &str, raw: &[u8], flags: u32, i: usize) -> Result<Vec<u8>> {
    let bad = |detail: String| Error::format(name, format!("segment {i}: {detail}"));
    let (foff, moff, size, csize) = segment_fields(raw, i).ok_or_else(|| bad("the header is cut short".into()))?;
    if moff.checked_add(size).is_none_or(|end| end > MAX_MODULE) {
        return Err(bad(format!("{size} bytes at {moff:#x}: larger than any module")));
    }
    // (a segment past the file's end is what is there of it, as the console's loader reads it: its size check fails)
    let stored = raw.get(foff..foff.saturating_add(csize).min(raw.len())).unwrap_or_default();
    let data = if flags & (1 << i) == 0 {
        stored.to_vec()
    } else if flags & ZBIC_FLAG != 0 {
        crate::zstd::decompress(crate::zstd::Dialect::Zbic, stored, size).map_err(|e| bad(format!("ZBIC: {e}")))?
    } else {
        let mut out = vec![0u8; size];
        let n = lz4_flex::block::decompress_into(stored, &mut out).map_err(|e| bad(format!("LZ4: {e}")))?;
        out.truncate(n);
        out
    };
    if data.len() != size {
        return Err(bad(format!("{} bytes, {size} expected", data.len())));
    }
    if flags & (8 << i) != 0 && raw.get(0xA0 + 0x20 * i..0xC0 + 0x20 * i) != Some(&Sha256::digest(&data)[..]) {
        return Err(bad("its SHA-256 is not the header's: the module is damaged".into()));
    }
    Ok(data)
}

/// The most symbols a module's table may hold (the game's main has some tens of thousands).
pub const MAX_SYMBOLS: u64 = 1 << 22;

/// How many symbols the table from `symtab` to `strtab` holds, its entries of `syment` bytes (0: the ELF64 size, 24):
/// an entry of another size, a table outside the image's `len` bytes or of more than [`MAX_SYMBOLS`] is refused.
fn symbol_count(strtab: u64, symtab: u64, syment: u64, len: u64) -> std::result::Result<u64, String> {
    if syment != 0 && syment != 24 {
        return Err(format!("symbol entries of {syment} bytes, not 24"));
    }
    if strtab <= symtab {
        return Ok(0);
    }
    let count = (strtab - symtab) / 24;
    if count > MAX_SYMBOLS || symtab.checked_add(count * 24).is_none_or(|end| end > len) {
        return Err(format!("a symbol table of {count} entries at {symtab:#x}, outside the module"));
    }
    Ok(count)
}

impl Nso {
    /// The module in the file at `path`.
    pub fn load(path: &Path) -> Result<Nso> {
        let raw = std::fs::read(path).at(path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Nso::parse(name, &raw)
    }

    /// The module of these bytes, named `name`.
    pub fn parse(name: String, raw: &[u8]) -> Result<Nso> {
        if raw.get(..4) != Some(b"NSO0") || raw.len() < 0x100 {
            return Err(Error::format(&name, "not an NSO"));
        }
        let flags = u32_at(raw, 0xC).unwrap_or(0);
        let mut datas = Vec::with_capacity(3);
        let mut end = 0usize;
        let mut segments = Vec::with_capacity(3);
        for (i, &seg_name) in SEGMENT_NAMES.iter().enumerate() {
            let data = segment_data(&name, raw, flags, i)?;
            let (_, moff, size, _) = segment_fields(raw, i).unwrap_or_default();
            end = end.max(moff + size); // (each under MAX_MODULE: segment_data checked)
            segments.push(Segment { name: seg_name, offset: moff as u64, size: size as u64 });
            datas.push((moff, data));
        }
        // (the whole image, the segments' end plus the header's bss size, is bounded before it is allocated: a
        // damaged header asking for gigabytes is refused, not aborted)
        let bss_size = u64::from(u32_at(raw, 0x3C).unwrap_or(0));
        if (end as u64).checked_add(bss_size).is_none_or(|whole| whole > MAX_MODULE as u64) {
            return Err(Error::format(
                &name,
                format!("the bss of {bss_size} bytes after {end:#x}: larger than any module"),
            ));
        }
        let mut image = vec![0u8; end];
        for (moff, data) in datas {
            image[moff..moff + data.len()].copy_from_slice(&data);
        }
        let id = &raw[0x40..0x60];
        let kept = id.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
        let build_id = build_core::key::to_hex(&id[..kept]);
        let segments: [Segment; 3] = segments.try_into().map_err(|_| Error::format(&name, "three segments"))?;
        let mut nso = Nso {
            name,
            build_id,
            bss_size,
            segments,
            image,
            mod0: 0,
            bss: (0, 0),
            dynamic: BTreeMap::new(),
            symbols: Vec::new(),
            relocs: Vec::new(),
            eh_funcs: Vec::new(),
        };
        nso.read_tables().map_err(|detail| Error::format(&nso.name, detail))?;
        Ok(nso)
    }

    /// The first value of a dynamic tag, or 0.
    pub fn dyn_value(&self, tag: i64) -> u64 {
        self.dynamic.get(&tag).and_then(|v| v.first()).copied().unwrap_or(0)
    }

    /// The code: `.text`'s offsets in the image.
    pub fn code_range(&self) -> (u64, u64) {
        let t = &self.segments[0];
        (t.offset, t.offset + t.size)
    }

    /// The offsets of DT_INIT_ARRAY's entries.
    pub fn init_array_offsets(&self) -> Result<Vec<u64>> {
        let start = self.dyn_value(DT_INIT_ARRAY);
        let size = self.dyn_value(DT_INIT_ARRAYSZ);
        // (the array's end inside the module bounds both its count and every offset, which stays below that end)
        let end = start.checked_add(size).filter(|&end| end <= self.image.len() as u64).ok_or_else(|| {
            Error::format(&self.name, format!("the init array {start:#x} for {size:#x} bytes is outside the module"))
        })?;
        let count = (end - start) / 8;
        Ok((0..count).map(|i| start + 8 * i).collect())
    }

    /// The symbol a relocation names.
    pub fn symbol_of(&self, r: &Reloc) -> Option<&Symbol> {
        r.symbol.map(|i| &self.symbols[i])
    }

    fn u64(&self, at: u64) -> std::result::Result<u64, String> {
        u64_at(&self.image, at).ok_or_else(|| format!("a table at {at:#x} is outside the module"))
    }

    fn read_tables(&mut self) -> std::result::Result<(), String> {
        // (each symbol read is checked against the image; the count, bounded first, keeps the list itself small)
        let m = u64::from(u32_at(&self.image, 4).ok_or("no MOD0 offset")?);
        if self.image.get(m as usize..m as usize + 4) != Some(b"MOD0") {
            return Err("no MOD0".into());
        }
        let rel = |k: u64| {
            i32_at(&self.image, m + 4 + 4 * k).and_then(|v| m.checked_add_signed(v)).ok_or("MOD0 cut short or before 0")
        };
        let (dynamic, bss_s, bss_e, eh_s, eh_e) = (rel(0)?, rel(1)?, rel(2)?, rel(3)?, rel(4)?);
        // (a bss before its start, or past any module's size: a damaged MOD0, said, not a panic or a huge allocation)
        if bss_s > bss_e || bss_e > MAX_MODULE as u64 {
            return Err(format!("the bss {bss_s:#x}..{bss_e:#x} is not inside a module"));
        }
        self.mod0 = m;
        self.bss = (bss_s, bss_e);
        if (self.image.len() as u64) < bss_e {
            self.image.resize(bss_e as usize, 0);
        }
        let mut at = dynamic;
        loop {
            let tag = self.u64(at)? as i64;
            let value = self.u64(at + 8)?;
            at += 16;
            if tag == 0 {
                break;
            }
            self.dynamic.entry(tag).or_default().push(value);
        }
        let (strtab, symtab) = (self.dyn_value(DT_STRTAB), self.dyn_value(DT_SYMTAB));
        let count = symbol_count(strtab, symtab, self.dyn_value(DT_SYMENT), self.image.len() as u64)?;
        for i in 0..count {
            let syment = 24;
            let at = symtab + i * syment;
            let b = self.image.get(at as usize..at as usize + 24).ok_or("the symbol table is outside the module")?;
            let name_off = u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            let (info, shndx) = (b[4], u16::from_le_bytes([b[6], b[7]]));
            let (value, size) = (self.u64(at + 8)?, self.u64(at + 16)?);
            self.symbols.push(Symbol {
                name: self.cstr(strtab + name_off)?,
                kind: info & 0xF,
                bind: info >> 4,
                defined: shndx != 0,
                value,
                size,
            });
        }
        for (start_tag, size_tag, what) in [(DT_RELA, DT_RELASZ, "RELA"), (DT_JMPREL, DT_PLTRELSZ, "JMPREL")] {
            let (start, size) = (self.dyn_value(start_tag), self.dyn_value(size_tag));
            let end = start.checked_add(size).filter(|&end| end <= self.image.len() as u64).ok_or_else(|| {
                format!("the {what} relocations {start:#x} for {size:#x} bytes are outside the module")
            })?;
            for at in (start..end).step_by(24) {
                let (offset, info, addend) = (self.u64(at)?, self.u64(at + 8)?, self.u64(at + 16)? as i64);
                let sym = (info >> 32) as usize;
                if sym >= self.symbols.len() && sym != 0 {
                    return Err(format!("a relocation names symbol {sym}, past the table"));
                }
                self.relocs.push(Reloc {
                    offset,
                    kind: RelocKind::of(info as u32),
                    symbol: (sym != 0).then_some(sym),
                    addend,
                });
            }
        }
        let (start, size) = (self.dyn_value(DT_RELR), self.dyn_value(DT_RELRSZ));
        if start != 0 && size != 0 {
            if !matches!(self.dyn_value(DT_RELRENT), 0 | 8) {
                return Err(format!("RELRENT {}", self.dyn_value(DT_RELRENT)));
            }
            for offset in self.relr_offsets(start, size)? {
                let addend = self.u64(offset)? as i64;
                self.relocs.push(Reloc { offset, kind: RelocKind::Relative, symbol: None, addend });
            }
        }
        self.eh_funcs = self.eh_frame_hdr_table(eh_s, eh_e);
        Ok(())
    }

    fn cstr(&self, at: u64) -> std::result::Result<String, String> {
        let tail = self.image.get(at as usize..).ok_or("a symbol name outside the module")?;
        let end = tail.iter().position(|&b| b == 0).ok_or("a symbol name without its end")?;
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    fn relr_offsets(&self, start: u64, size: u64) -> std::result::Result<Vec<u64>, String> {
        let mut out = Vec::new();
        let mut cursor = 0u64;
        let end = start
            .checked_add(size)
            .filter(|&end| end <= self.image.len() as u64)
            .ok_or_else(|| format!("the RELR relocations {start:#x} for {size:#x} bytes are outside the module"))?;
        for at in (start..end).step_by(8) {
            let e = self.u64(at)?;
            if e & 1 == 0 {
                out.push(e);
                cursor = e.checked_add(8).ok_or_else(|| format!("a RELR entry {e:#x} overflows its next address"))?;
            } else {
                let mut bits = e >> 1;
                let mut i = 0;
                while bits != 0 {
                    if bits & 1 != 0 {
                        let offset = cursor
                            .checked_add(8 * i)
                            .ok_or_else(|| format!("a RELR bitmap at {e:#x} overflows its address"))?;
                        out.push(offset);
                    }
                    bits >>= 1;
                    i += 1;
                }
                cursor = cursor
                    .checked_add(63 * 8)
                    .ok_or_else(|| format!("a RELR bitmap at {e:#x} overflows its cursor"))?;
            }
        }
        Ok(out)
    }

    /// Function starts from `.eh_frame_hdr`'s binary-search table (datarel sdata4 entries); none for another form.
    fn eh_frame_hdr_table(&self, s: u64, e: u64) -> Vec<u64> {
        if e <= s {
            return Vec::new();
        }
        let Some(h) = self.image.get(s as usize..s as usize + 4) else { return Vec::new() };
        let (version, ptr_enc, count_enc, table_enc) = (h[0], h[1], h[2], h[3]);
        if version != 1 || table_enc != 0x3B || count_enc != 0x03 || !matches!(ptr_enc & 0x0F, 0x0B | 0x03) {
            return Vec::new();
        }
        let Some(count) = u32_at(&self.image, s as usize + 8) else { return Vec::new() };
        (0..u64::from(count))
            .map_while(|i| i32_at(&self.image, s + 12 + 8 * i).map(|v| s.wrapping_add_signed(v)))
            .collect()
    }
}

/// The modules of an exefs folder, in [`MODULE_ORDER`]: those it has.
pub fn load_exefs(exefs: &Path) -> Result<Vec<Nso>> {
    MODULE_ORDER.iter().map(|n| exefs.join(n)).filter(|p| p.exists()).map(|p| Nso::load(&p)).collect()
}

/// A module's build id from its header, all 32 bytes as hex (a plain or a compressed module: the header is the same);
/// `None` when the file is not an NSO.
pub fn build_id(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    let mut head = [0u8; 0x60];
    let mut f = std::fs::File::open(path).at(path)?;
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]).at(path)? {
            0 => break,
            k => n += k,
        }
    }
    if n < head.len() || head[..4] != *b"NSO0" {
        return Ok(None);
    }
    Ok(Some(build_core::key::to_hex(&head[0x40..0x60])))
}

/// What [`unpack_in_place`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unpacked {
    /// Not an NSO: left as it is.
    NotNso,
    /// Not ZBIC: left as it is.
    Kept,
    /// Rewritten as a plain NSO.
    Plain,
}

/// The module at `path`, if its segments are ZBIC, rewritten as a plain NSO (its segments uncompressed, the header's
/// hash bits kept: the hashes are of the plain bytes), so every tool reads it as before firmware 22.
pub fn unpack_in_place(path: &Path) -> Result<Unpacked> {
    let raw = std::fs::read(path).at(path)?;
    if raw.get(..4) != Some(b"NSO0") || raw.len() < 0x100 {
        return Ok(Unpacked::NotNso);
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(match plain(&name, &raw)? {
        None => Unpacked::Kept,
        Some(bytes) => {
            build_core::fsx::write_atomic(path, &bytes)?;
            Unpacked::Plain
        }
    })
}

/// A ZBIC module's bytes as a plain NSO, or `None` when it is not ZBIC.
pub fn plain(name: &str, raw: &[u8]) -> Result<Option<Vec<u8>>> {
    let flags = u32_at(raw, 0xC).unwrap_or(0);
    if flags & ZBIC_FLAG == 0 {
        return Ok(None);
    }
    let fields: Vec<_> = (0..3)
        .map(|i| segment_fields(raw, i))
        .collect::<Option<_>>()
        .ok_or_else(|| Error::format(name, "header cut short"))?;
    let datas = (0..3).map(|i| segment_data(name, raw, flags, i)).collect::<Result<Vec<_>>>()?;
    let mut header = raw[..0x100].to_vec();
    // (what lies between the header and the first segment: the module's name, which the header points at)
    let first = fields.iter().map(|f| f.0).min().unwrap_or(0x100).clamp(0x100, raw.len());
    let mut body = raw[0x100..first].to_vec();
    for (i, d) in datas.iter().enumerate() {
        header[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&((0x100 + body.len()) as u32).to_le_bytes());
        header[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(d.len() as u32).to_le_bytes());
        body.extend_from_slice(d);
    }
    header[0xC..0x10].copy_from_slice(&(flags & !(ZBIC_FLAG | 7)).to_le_bytes());
    header.extend_from_slice(&body);
    Ok(Some(header))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small module: code, rodata and data, a MOD0, a dynamic section with symbols, RELA, JMPREL and RELR, and an
    /// `.eh_frame_hdr` table. `compress`: its segments as LZ4.
    pub(crate) fn synthetic(build_id: u8, compress: bool) -> Vec<u8> {
        // .text 0..0x100 (MOD0, code), .rodata 0x100..0x300 (symbols 0x140, strings 0x170, RELA 0x180, JMPREL 0x198,
        // RELR 0x1b0, dynamic 0x200, eh_frame_hdr 0x2e0), .data 0x300..0x340, bss 0x340..0x380
        let mut img = vec![0u8; 0x340];
        fn w32(img: &mut [u8], at: usize, v: u32) {
            img[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        fn w64(img: &mut [u8], at: usize, v: u64) {
            img[at..at + 8].copy_from_slice(&v.to_le_bytes());
        }
        let rel = |to: i64| ((to - 0x10) as i32) as u32;
        w32(&mut img, 4, 0x10);
        img[0x10..0x14].copy_from_slice(b"MOD0");
        for (k, to) in [0x200, 0x340, 0x380, 0x2e0, 0x300].into_iter().enumerate() {
            w32(&mut img, 0x14 + 4 * k, rel(to));
        }
        for i in 0..8 {
            w32(&mut img, 0x40 + 4 * i, 0xd503201f); // nop
        }
        w32(&mut img, 0x60, 0x94000004); // bl +16
        w32(&mut img, 0x70, 0xd65f03c0); // ret
        // symbols: [0] none, [1] "exp", a global function defined at 0x60
        img[0x15c] = 0x12;
        img[0x15e] = 1;
        w32(&mut img, 0x158, 1);
        w64(&mut img, 0x160, 0x60);
        img[0x170..0x175].copy_from_slice(b"\0exp\0");
        // RELA: RELATIVE at 0x300, addend 0x70; JMPREL: JUMP_SLOT of symbol 1 at 0x308
        w64(&mut img, 0x180, 0x300);
        w64(&mut img, 0x188, 1027);
        w64(&mut img, 0x190, 0x70);
        w64(&mut img, 0x198, 0x308);
        w64(&mut img, 0x1a0, (1u64 << 32) | 1026);
        // RELR: the word at 0x310, then a bitmap from 0x318 whose bit 1 is 0x320; their addends are the words
        w64(&mut img, 0x1b0, 0x310);
        w64(&mut img, 0x1b8, (1 << 2) | 1);
        w64(&mut img, 0x310, 0x40);
        w64(&mut img, 0x320, 0x44);
        let dynamic: [(i64, u64); 11] = [
            (DT_SYMTAB, 0x140),
            (DT_STRTAB, 0x170),
            (DT_STRSZ, 0x10),
            (DT_SYMENT, 24),
            (DT_RELA, 0x180),
            (DT_RELASZ, 24),
            (DT_JMPREL, 0x198),
            (DT_PLTRELSZ, 24),
            (DT_RELR, 0x1b0),
            (DT_RELRSZ, 16),
            (DT_INIT, 0x40),
        ];
        for (i, (t, v)) in dynamic.into_iter().enumerate() {
            w64(&mut img, 0x200 + 16 * i, t as u64);
            w64(&mut img, 0x208 + 16 * i, v);
        }
        // eh_frame_hdr: version 1, pcrel sdata4 pointer, udata4 count, datarel sdata4 table: two functions
        img[0x2e0..0x2e4].copy_from_slice(&[1, 0x1b, 0x03, 0x3b]);
        w32(&mut img, 0x2e8, 2);
        w32(&mut img, 0x2ec, (0x40i32 - 0x2e0) as u32);
        w32(&mut img, 0x2f4, (0x60i32 - 0x2e0) as u32);
        let img = &img[..];
        let segs = [(0usize, 0x100usize), (0x100, 0x200), (0x300, 0x40)];
        let mut raw = vec![0u8; 0x100];
        raw[..4].copy_from_slice(b"NSO0");
        let mut flags = 0u32;
        let mut body = Vec::new();
        for (i, &(moff, size)) in segs.iter().enumerate() {
            let plain = &img[moff..moff + size];
            let stored = if compress { lz4_flex::block::compress(plain) } else { plain.to_vec() };
            if compress {
                flags |= 1 << i;
            }
            flags |= 8 << i;
            raw[0xA0 + 0x20 * i..0xC0 + 0x20 * i].copy_from_slice(&Sha256::digest(plain));
            raw[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&((0x100 + body.len()) as u32).to_le_bytes());
            raw[0x14 + 0x10 * i..0x18 + 0x10 * i].copy_from_slice(&(moff as u32).to_le_bytes());
            raw[0x18 + 0x10 * i..0x1c + 0x10 * i].copy_from_slice(&(size as u32).to_le_bytes());
            raw[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(stored.len() as u32).to_le_bytes());
            body.extend_from_slice(&stored);
        }
        raw[0xC..0x10].copy_from_slice(&flags.to_le_bytes());
        raw[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        raw[0x40] = build_id;
        raw.extend_from_slice(&body);
        raw
    }

    #[test]
    fn a_symbol_table_of_another_entry_size_or_too_big_is_refused() {
        assert_eq!(symbol_count(0x1000 + 48, 0x1000, 0, 0x2000), Ok(2));
        assert_eq!(symbol_count(0x1000 + 48, 0x1000, 24, 0x2000), Ok(2));
        assert!(symbol_count(0x1000 + 48, 0x1000, 1, 0x2000).is_err(), "1-byte entries: 48 symbols of one table");
        assert!(symbol_count(0x10_0000, 0x1000, 24, 0x2000).is_err(), "past the image");
        assert!(symbol_count((MAX_SYMBOLS + 1) * 24, 0, 24, u64::MAX).is_err());
        assert_eq!(symbol_count(0x1000, 0x2000, 24, 0x2000), Ok(0));
    }

    #[test]
    fn a_module_s_sizes_are_bounded_before_anything_is_allocated() {
        let good = synthetic(1, false);
        assert!(Nso::parse("m".into(), &good).is_ok());
        // (the .data segment's size field as 4 GB, then its memory offset past any module)
        for (at, v) in [(0x38usize, u32::MAX), (0x34, 0xF000_0000)] {
            let mut raw = good.clone();
            raw[at..at + 4].copy_from_slice(&v.to_le_bytes());
            assert!(Nso::parse("m".into(), &raw).is_err(), "field {at:#x} = {v:#x}");
        }
    }

    #[test]
    fn a_bss_before_its_start_is_said_not_a_panic() {
        // (MOD0 at 0x10: its bss end, field 2, made to point before address 0, and before the bss start)
        for v in [i32::MIN, -0x10] {
            let mut raw = synthetic(1, false);
            let mut nso_img_field = None;
            for i in 0..raw.len() - 4 {
                if raw[i..i + 4] == *b"MOD0" {
                    nso_img_field = Some(i + 4 + 4 * 2);
                    break;
                }
            }
            let at = nso_img_field.unwrap();
            raw[at..at + 4].copy_from_slice(&v.to_le_bytes());
            raw[0xC] &= !8; // (.text's hash not checked: the edit is to be read, not refused as damage)
            assert!(Nso::parse("m".into(), &raw).is_err(), "bss end {v}");
        }
    }

    #[test]
    fn an_untrusted_bss_size_is_bounded() {
        // (the header's bss size at 0x3C, past any module: the image's end plus it would be about 4 GB)
        let mut raw = synthetic(1, false);
        raw[0x3C..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Nso::parse("m".into(), &raw).is_err(), "bss size {:#x}", u32::MAX);
    }

    #[test]
    fn an_untrusted_relocation_table_range_is_refused() {
        // (DT_RELASZ, image 0x258 (raw 0x358), made to overflow DT_RELA + its size)
        let mut raw = synthetic(1, false);
        raw[0x358..0x360].copy_from_slice(&u64::MAX.to_le_bytes());
        raw[0xC] &= !(8 << 1); // (.rodata's hash not checked: the edit is to be read, not refused as damage)
        assert!(Nso::parse("m".into(), &raw).is_err(), "DT_RELASZ {:#x}", u64::MAX);
    }

    #[test]
    fn an_untrusted_relr_entry_is_refused() {
        // (the first RELR entry, image 0x1b0 (raw 0x2b0): an even address of u64::MAX - 1, whose next overflows)
        let mut raw = synthetic(1, false);
        raw[0x2b0..0x2b8].copy_from_slice(&(u64::MAX - 1).to_le_bytes());
        raw[0xC] &= !(8 << 1); // (.rodata's hash not checked: the edit is to be read, not refused as damage)
        assert!(Nso::parse("m".into(), &raw).is_err(), "RELR entry {:#x}", u64::MAX - 1);
    }

    #[test]
    fn an_untrusted_init_array_is_refused() {
        // (DT_INIT_ARRAY and DT_INIT_ARRAYSZ where the dynamic section's terminator was, image 0x2b0..0x2d0 (raw
        // 0x3b0..0x3d0): a start near u64::MAX and 0x100 bytes, so the count and every offset walk off the module)
        let mut raw = synthetic(1, false);
        for (at, v) in [
            (0x3b0usize, DT_INIT_ARRAY as u64),
            (0x3b8, u64::MAX - 8),
            (0x3c0, DT_INIT_ARRAYSZ as u64),
            (0x3c8, 0x100),
            (0x3d0, 0),
        ] {
            raw[at..at + 8].copy_from_slice(&v.to_le_bytes());
        }
        raw[0xC] &= !(8 << 1); // (.rodata's hash not checked: the edit is to be read, not refused as damage)
        let n = Nso::parse("m".into(), &raw).unwrap();
        assert!(n.init_array_offsets().is_err(), "DT_INIT_ARRAY {:#x}", u64::MAX - 8);
    }

    #[test]
    fn a_module_s_tables() {
        for compress in [false, true] {
            let n = Nso::parse("main".into(), &synthetic(0xe0, compress)).unwrap();
            assert_eq!(n.build_id, "e0");
            assert_eq!(n.image.len(), 0x380, "the image reaches the bss's end");
            assert_eq!(n.code_range(), (0, 0x100));
            assert_eq!(n.dyn_value(DT_INIT), 0x40);
            assert_eq!(n.symbols.len(), 2);
            assert_eq!(
                (n.symbols[1].name.as_str(), n.symbols[1].is_function(), n.symbols[1].value),
                ("exp", true, 0x60)
            );
            let relocs: Vec<(u64, RelocKind, i64)> = n.relocs.iter().map(|r| (r.offset, r.kind, r.addend)).collect();
            assert_eq!(
                relocs,
                [
                    (0x300, RelocKind::Relative, 0x70),
                    (0x308, RelocKind::JumpSlot, 0),
                    (0x310, RelocKind::Relative, 0x40),
                    (0x320, RelocKind::Relative, 0x44)
                ]
            );
            assert_eq!(n.eh_funcs, [0x40, 0x60]);
        }
    }

    #[test]
    fn a_damaged_segment_is_refused() {
        let mut raw = synthetic(1, false);
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(Nso::parse("main".into(), &raw).is_err());
        assert!(Nso::parse("x".into(), b"NSO1").is_err());
    }

    #[test]
    fn a_module_that_is_not_zbic_is_kept() {
        assert_eq!(plain("main", &synthetic(1, true)).unwrap(), None);
    }
}

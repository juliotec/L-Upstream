//! The game's image as the lift lays it out: every module at the address Eden loads it to, from the image's base up,
//! made from the exefs by [`build`] at build time and written as `image.bin` (the memory) and `image.json` (its
//! modules, segments, exports and HLE hooks), then read here.
//!
//! Instructions are read from here. Code is a module's `.text`: an address is code when it is 4-aligned and inside one.
//! This file is the lifter's key (lift/key_crates.rs); [`build`], which makes the files, is not: the lift reads what it
//! writes, not its code.

pub mod build;

use serde::Deserialize;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// What can go wrong reading an image.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file could not be read.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// `image.json` is not what [`build`] writes.
    #[error("{path}: {source}")]
    Json {
        /// The file.
        path: PathBuf,
        /// Why.
        source: serde_json::Error,
    },
    /// The metadata and the memory disagree, or an address range runs past 2^64.
    #[error("{0}")]
    Layout(String),
}

/// A part of a module: `.text`, `.rodata` or `.data`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Segment {
    /// Its name, as the NSO names it.
    pub name: String,
    /// Its first address.
    pub addr: u64,
    /// Its size in bytes.
    pub size: u64,
}

impl Segment {
    /// The addresses it covers (an image's segments end below 2^64: [`Image::new`] checks).
    pub fn range(&self) -> Range<u64> {
        self.addr..self.addr.saturating_add(self.size)
    }
}

/// One NSO module (rtld, main, subsdk*, sdk) where it is loaded.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Module {
    /// rtld, main, subsdk0 ...
    pub name: String,
    /// Where it is loaded.
    pub base: u64,
    /// Its size in the image, bss included.
    pub size: u64,
    /// Its build id (hex), as its header gives it.
    #[serde(default)]
    pub build_id: String,
    /// Its segments.
    pub segments: Vec<Segment>,
}

impl Module {
    /// Its code: the `.text` segment, if it has one.
    pub fn text(&self) -> Option<Range<u64>> {
        self.segments.iter().find(|s| s.name == ".text").map(Segment::range)
    }
}

/// An address the host implements (an HLE function or a stub): the lift calls the host there.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Hook {
    /// The guest address.
    pub addr: u64,
    /// The host function's C name.
    #[serde(rename = "c")]
    pub name: String,
    /// The SDK symbol it stands for.
    pub sym: String,
}

/// A module's record in `image.json`: the module and its DT_INIT.
#[derive(Deserialize)]
struct ModuleMeta {
    #[serde(flatten)]
    module: Module,
    #[serde(default)]
    init: u64,
}

#[derive(Deserialize)]
struct Meta {
    base: u64,
    end: u64,
    modules: Vec<ModuleMeta>,
    #[serde(default)]
    hooks: Vec<Hook>,
    #[serde(default)]
    exports: BTreeMap<String, u64>,
}

/// The image: memory from `base` to `end`, and what the metadata says of it.
#[derive(Clone, Debug)]
pub struct Image {
    base: u64,
    mem: Vec<u8>,
    modules: Vec<Module>,
    code: Vec<Range<u64>>,
    hooks: Vec<Hook>,
    exports: BTreeMap<String, u64>,
    inits: Vec<u64>,
}

impl Image {
    /// The image a lift wrote into `dir` (its `image.bin` and `image.json`).
    pub fn load(dir: &Path) -> Result<Image, Error> {
        let bin = dir.join("image.bin");
        let json = dir.join("image.json");
        let mem = std::fs::read(&bin).map_err(|source| Error::Io { path: bin.clone(), source })?;
        let text = std::fs::read_to_string(&json).map_err(|source| Error::Io { path: json.clone(), source })?;
        let meta: Meta = serde_json::from_str(&text).map_err(|source| Error::Json { path: json.clone(), source })?;
        if meta.end.checked_sub(meta.base) != Some(mem.len() as u64) {
            return Err(Error::Layout(format!(
                "{}: {} bytes, where {} says {:#x}..{:#x}",
                bin.display(),
                mem.len(),
                json.display(),
                meta.base,
                meta.end
            )));
        }
        let mut metas = meta.modules;
        metas.sort_by_key(|m| m.module.base);
        let inits = metas.iter().map(|m| m.init).collect();
        let modules = metas.into_iter().map(|m| m.module).collect();
        Ok(Image::new(meta.base, mem, modules)?.with_tables(meta.hooks, meta.exports, inits))
    }

    /// An image of `mem` from `base`, with these modules (each module's `.text` within the memory), kept in address
    /// order.
    pub fn new(base: u64, mem: Vec<u8>, mut modules: Vec<Module>) -> Result<Image, Error> {
        let end = base
            .checked_add(mem.len() as u64)
            .ok_or_else(|| Error::Layout(format!("{} bytes from {base:#x} run past 2^64", mem.len())))?;
        if let Some(s) = modules.iter().flat_map(|m| &m.segments).find(|s| s.addr.checked_add(s.size).is_none()) {
            return Err(Error::Layout(format!("{}: {:#x} bytes from {:#x} run past 2^64", s.name, s.size, s.addr)));
        }
        modules.sort_by_key(|m| m.base);
        let mut code: Vec<Range<u64>> = modules.iter().filter_map(Module::text).collect();
        code.sort_by_key(|r| r.start);
        if let Some(bad) = code.iter().find(|r| r.start < base || r.end > end) {
            return Err(Error::Layout(format!(
                "code {:#x}..{:#x} outside the image {base:#x}..{end:#x}",
                bad.start, bad.end
            )));
        }
        let inits = vec![0; modules.len()];
        Ok(Image { base, mem, modules, code, hooks: Vec::new(), exports: BTreeMap::new(), inits })
    }

    /// This image with the metadata's tables: the host's functions at guest addresses, the exported symbols, and each
    /// module's DT_INIT (an absolute address, 0 for none), in module (address) order.
    pub fn with_tables(mut self, hooks: Vec<Hook>, exports: BTreeMap<String, u64>, mut inits: Vec<u64>) -> Image {
        inits.resize(self.modules.len(), 0);
        self.hooks = hooks;
        self.exports = exports;
        self.inits = inits;
        self
    }

    /// Each module's DT_INIT, an absolute address (0 for none), in module (address) order.
    pub fn inits(&self) -> &[u64] {
        &self.inits
    }

    /// The first address.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The address after the last.
    pub fn end(&self) -> u64 {
        self.base + self.mem.len() as u64
    }

    /// The modules, in address order.
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// The code ranges (each module's `.text`), in address order.
    pub fn code(&self) -> &[Range<u64>] {
        &self.code
    }

    /// The host's functions at guest addresses.
    pub fn hooks(&self) -> &[Hook] {
        &self.hooks
    }

    /// The modules' exported functions, by symbol.
    pub fn exports(&self) -> &BTreeMap<String, u64> {
        &self.exports
    }

    /// The module `name` (main, sdk ...).
    pub fn module(&self, name: &str) -> Option<&Module> {
        self.modules.iter().find(|m| m.name == name)
    }

    /// Whether `addr` is an instruction's address: 4-aligned, in a module's code.
    pub fn in_code(&self, addr: u64) -> bool {
        if addr & 3 != 0 {
            return false;
        }
        let i = self.code.partition_point(|r| r.end <= addr);
        self.code.get(i).is_some_and(|r| r.contains(&addr))
    }

    /// `len` bytes at `addr`, or `None` past the image.
    pub fn read(&self, addr: u64, len: usize) -> Option<&[u8]> {
        let off = usize::try_from(addr.checked_sub(self.base)?).ok()?;
        self.mem.get(off..off.checked_add(len)?)
    }

    /// The 32-bit little-endian word at `addr`, or `None` past the image.
    pub fn word(&self, addr: u64) -> Option<u32> {
        self.read(addr, 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The 64-bit little-endian word at `addr`, or `None` past the image.
    pub fn u64(&self, addr: u64) -> Option<u64> {
        let b = self.read(addr, 8)?;
        Some(u64::from_le_bytes(b.try_into().ok()?))
    }

    /// The instruction at `addr`: its word, when `addr` is code.
    pub fn insn(&self, addr: u64) -> Option<u32> {
        if self.in_code(addr) { self.word(addr) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Image {
        let module = Module {
            name: "main".into(),
            base: 0x1000,
            size: 0x20,
            build_id: String::new(),
            segments: vec![
                Segment { name: ".text".into(), addr: 0x1000, size: 0x10 },
                Segment { name: ".data".into(), addr: 0x1010, size: 0x10 },
            ],
        };
        let mut mem = vec![0u8; 0x20];
        mem[4..8].copy_from_slice(&0xd65f03c0u32.to_le_bytes());
        Image::new(0x1000, mem, vec![module]).unwrap()
    }

    #[test]
    fn code_and_words() {
        let img = tiny();
        assert!(img.in_code(0x1000) && img.in_code(0x100c));
        assert!(!img.in_code(0x1010) && !img.in_code(0x1002) && !img.in_code(0xffc));
        assert_eq!(img.insn(0x1004), Some(0xd65f03c0));
        assert_eq!(img.insn(0x1014), None);
        assert_eq!(img.word(0x1014), Some(0));
        assert_eq!(img.word(0x101e), None);
        assert_eq!(img.read(0x0fff, 1), None);
    }

    #[test]
    fn code_outside_the_memory_is_refused() {
        let module = Module {
            name: "m".into(),
            base: 0,
            size: 0,
            build_id: String::new(),
            segments: vec![Segment { name: ".text".into(), addr: 0x10, size: 0x10 }],
        };
        assert!(matches!(Image::new(0, vec![0; 0x18], vec![module]), Err(Error::Layout(_))));
    }

    fn module(name: &str, base: u64, text: (u64, u64)) -> Module {
        let segments = vec![Segment { name: ".text".into(), addr: text.0, size: text.1 }];
        Module { name: name.into(), base, size: text.1, build_id: String::new(), segments }
    }

    #[test]
    fn modules_are_in_address_order() {
        let img =
            Image::new(0x1000, vec![0; 0x40], vec![module("b", 0x1020, (0x1020, 8)), module("a", 0x1000, (0x1000, 8))])
                .unwrap();
        let names: Vec<&str> = img.modules().iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn ranges_past_the_address_space_are_refused() {
        assert!(matches!(Image::new(u64::MAX - 4, vec![0; 0x10], Vec::new()), Err(Error::Layout(_))));
        let wraps = module("m", 0, (u64::MAX - 4, 0x10));
        assert!(matches!(Image::new(0, vec![0; 0x10], vec![wraps]), Err(Error::Layout(_))));
    }
}

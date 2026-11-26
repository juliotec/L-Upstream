//! The game's image: its code modules laid out where the emulator (Eden) loads them, the mods' ExeFS patches written
//! into main, every relocation applied and every symbol import bound, once, at build time. The lift translates its
//! code; the game loads it whole at run time (`image.bin`, read into guest memory by host/hos_main.c), so a change
//! of the image's data alone (a data-only ExeFS patch) needs no compile: only this file is written again.
//!
//! - The modules (rtld, main, subsdk0..9, sdk, those the exefs has) load back to back from 0x80000000 plus the ASLR
//!   offset (a 39-bit process), each taking its `.data` end plus the header's bss size, page aligned (Eden's
//!   loader/nso.cpp LoadModule); rtld 0x9000 more when program arguments are set.
//! - Symbols resolve in that order, the first definition winning. A symbol of the HLE set a loaded module defines is
//!   hooked at its definition (the lift calls the host there). An undefined weak symbol resolves to 0, as rtld does,
//!   unless it is in the set; any other unresolved symbol binds to a trampoline (16-byte slots from 0x07000000, no
//!   code: a call there calls the host's `imp_<name>`), so a call to it stops with its name instead of jumping to 0.
//! - Relocations: RELATIVE (and DT_RELR's) write base + addend; ABS64, GLOB_DAT and JUMP_SLOT symbol + addend (the
//!   AArch64 ELF ABI's S + A, as rtld writes them). Any other type stops the build.
//!
//! `image.json` says what is where: the modules, their segments and bss, the hooks, the unresolved imports, each
//! module's init array (relocated), and the exported functions.

use build_core::{Error, IoContext, Result};
use game_files::nso::{DT_FINI, DT_INIT, Nso, RelocKind};
use game_files::patches::{self, Applied, PatchFile};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// A 39-bit process's code address, before the ASLR offset.
pub const CODE_39BIT: u64 = 0x8000_0000;
/// Where the trampolines of unresolved imports are.
pub const TRAMP_BASE: u64 = 0x0700_0000;
/// rtld's argument block, when program arguments are set.
pub const ARGS_SIZE: u64 = 0x9000;
const PAGE: u64 = 0x1000;

/// How the image is laid out and bound.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The ASLR offset recomp runs are started with ((rng_seed << 12) & 0xfff000).
    pub aslr: u64,
    /// Whether rtld takes room for program arguments.
    pub program_args: bool,
    /// The symbols the host implements (HLE functions and stubs).
    pub hle: BTreeSet<String>,
}

/// A module where it is loaded.
#[derive(Clone, Debug)]
pub struct Placed {
    /// The module (main's image patched).
    pub nso: Nso,
    /// Its address.
    pub base: u64,
}

/// The built image.
#[derive(Clone, Debug)]
pub struct Image {
    /// The first address.
    pub base: u64,
    /// The address after the last module.
    pub end: u64,
    /// The memory from `base` to `end`.
    pub mem: Vec<u8>,
    /// The modules, in load order.
    pub modules: Vec<Placed>,
    /// The host's functions at guest addresses: address to C name.
    pub hooks: BTreeMap<u64, String>,
    hook_syms: HashMap<String, String>,
    tramps: HashMap<String, u64>,
    /// Imports no module defines, with how often they are referenced, in the order first met.
    pub unresolved: Vec<(String, u32)>,
    /// Each module's init array, relocated.
    pub init_arrays: Vec<(String, Vec<u64>)>,
    /// The exported functions, by symbol, in the order first defined.
    pub exports: Vec<(String, u64)>,
    /// What the ExeFS patches did.
    pub patched: Applied,
}

/// A symbol's host function name: `imp_` and its letters, digits and underscores, any other character an underscore.
pub fn c_ident(sym: &str) -> String {
    "imp_".chars().chain(sym.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })).collect()
}

fn page_align(v: u64) -> u64 {
    v.div_ceil(PAGE) * PAGE
}

/// LoadModule's size for a module: its `.data` end plus the header's bss size, page aligned.
pub fn eden_size(m: &Nso) -> u64 {
    let d = &m.segments[2];
    page_align(d.offset + d.size + m.bss_size)
}

impl Image {
    /// The image of these modules (in load order), with these patch files' writes in main.
    pub fn build(modules: Vec<Nso>, files: &[PatchFile], opts: &Options) -> Result<Image> {
        let mut base = CODE_39BIT + opts.aslr;
        let start = base;
        let mut placed = Vec::with_capacity(modules.len());
        for mut m in modules {
            let size = eden_size(&m) + if m.name == "rtld" && opts.program_args { ARGS_SIZE } else { 0 };
            if m.image.len() as u64 > size {
                return Err(Error::format(&m.name, "the module's image is larger than the room its header gives it"));
            }
            m.image.resize(size as usize, 0);
            placed.push(Placed { nso: m, base });
            base += size;
        }
        let mut patched = Applied::default();
        if let Some(main) = placed.iter_mut().find(|p| p.nso.name == "main") {
            patched = patches::apply(&mut main.nso, files)?;
        }
        let mut mem = Vec::with_capacity((base - start) as usize);
        for p in &placed {
            mem.extend_from_slice(&p.nso.image);
        }
        let mut img = Image {
            base: start,
            end: base,
            mem,
            modules: placed,
            hooks: BTreeMap::new(),
            hook_syms: HashMap::new(),
            tramps: HashMap::new(),
            unresolved: Vec::new(),
            init_arrays: Vec::new(),
            exports: Vec::new(),
            patched,
        };
        // (the first definition of each name, over the modules in load order)
        let mut defined: HashMap<&str, (usize, usize)> = HashMap::new();
        let mut order = Vec::new();
        for (mi, p) in img.modules.iter().enumerate() {
            for (si, s) in p.nso.symbols.iter().enumerate() {
                if s.defined && !s.name.is_empty() && !defined.contains_key(s.name.as_str()) {
                    defined.insert(s.name.as_str(), (mi, si));
                    order.push((mi, si));
                }
            }
        }
        let address = |&(mi, si): &(usize, usize)| img.modules[mi].base + img.modules[mi].nso.symbols[si].value;
        let exports: HashMap<String, u64> = defined.iter().map(|(n, at)| (n.to_string(), address(at))).collect();
        img.exports = order
            .iter()
            .filter(|&&(mi, si)| img.modules[mi].nso.symbols[si].is_function())
            .map(|at| (img.modules[at.0].nso.symbols[at.1].name.clone(), address(at)))
            .collect();
        drop(defined);
        // (an HLE'd symbol a module defines is hooked at its definition: the game's import resolves to it as usual,
        // and lifted SDK code calling it inside the module reaches the same host function)
        for sym in &opts.hle {
            if let Some(&a) = exports.get(sym) {
                let name = c_ident(sym);
                img.hooks.insert(a, name.clone());
                img.hook_syms.insert(name, sym.clone());
            }
        }
        img.relocate(&exports, &opts.hle)?;
        for mi in 0..img.modules.len() {
            let p = &img.modules[mi];
            let words: Vec<u64> =
                p.nso.init_array_offsets()?.iter().map(|&off| img.u64_at(p.base + off)).collect::<Result<_>>()?;
            img.init_arrays.push((p.nso.name.clone(), words));
        }
        Ok(img)
    }

    fn u64_at(&self, addr: u64) -> Result<u64> {
        let at = addr.checked_sub(self.base).map(|o| o as usize);
        at.and_then(|o| self.mem.get(o..o + 8))
            .map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()))
            .ok_or_else(|| Error::format("image", format!("a word at {addr:#x} is outside the image")))
    }

    fn trampoline(&mut self, sym: &str) -> u64 {
        if let Some(&a) = self.tramps.get(sym) {
            return a;
        }
        let a = TRAMP_BASE + 16 * self.tramps.len() as u64;
        self.tramps.insert(sym.to_string(), a);
        let name = c_ident(sym);
        self.hooks.insert(a, name.clone());
        self.hook_syms.insert(name, sym.to_string());
        a
    }

    fn resolve(&mut self, sym: &str, weak: bool, exports: &HashMap<String, u64>, hle: &BTreeSet<String>) -> u64 {
        if let Some(&a) = exports.get(sym) {
            return a;
        }
        let in_hle = hle.contains(sym);
        if weak && !in_hle {
            return 0;
        }
        if !in_hle {
            match self.unresolved.iter_mut().find(|u| u.0 == sym) {
                Some(u) => u.1 += 1,
                None => self.unresolved.push((sym.to_string(), 1)),
            }
        }
        self.trampoline(sym)
    }

    fn relocate(&mut self, exports: &HashMap<String, u64>, hle: &BTreeSet<String>) -> Result<()> {
        for mi in 0..self.modules.len() {
            let (base, name) = (self.modules[mi].base, self.modules[mi].nso.name.clone());
            // (a name's binding in this module: its last symbol of that name)
            let weak: HashMap<String, bool> =
                self.modules[mi].nso.symbols.iter().map(|s| (s.name.clone(), s.is_weak())).collect();
            let relocs = std::mem::take(&mut self.modules[mi].nso.relocs);
            for r in &relocs {
                // (each word inside its own module: a damaged NSO's offset past its end is refused, never written
                // over the next module)
                let module_len = self.modules[mi].nso.image.len() as u64;
                if r.offset.checked_add(8).is_none_or(|end| end > module_len) {
                    return Err(Error::format(
                        &name,
                        format!(
                            "a relocation at {:#x} is outside the module ({module_len:#x} bytes): it is damaged",
                            r.offset
                        ),
                    ));
                }
                let v = match r.kind {
                    RelocKind::Relative => base.checked_add_signed(r.addend).ok_or_else(|| {
                        Error::format(&name, format!("a relative relocation at {:#x} past the address space", r.offset))
                    })?,
                    RelocKind::Abs64 | RelocKind::GlobDat | RelocKind::JumpSlot => {
                        let sym = self.modules[mi].nso.symbol_of(r).map(|s| s.name.clone()).ok_or_else(|| {
                            Error::format(&name, format!("a symbol relocation at {:#x} without a symbol", r.offset))
                        })?;
                        let w = weak.get(&sym).copied().unwrap_or(false);
                        self.resolve(&sym, w, exports, hle).wrapping_add_signed(r.addend)
                    }
                    RelocKind::Other(t) => {
                        return Err(Error::format(
                            &name,
                            format!("relocation type {t} at {:#x} not supported", r.offset),
                        ));
                    }
                };
                let at = base.checked_add(r.offset).and_then(|a| a.checked_sub(self.base)).map(|a| a as usize);
                let slot = at.and_then(|at| self.mem.get_mut(at..at.checked_add(8)?)).ok_or_else(|| {
                    Error::format(&name, format!("a relocation at {:#x} is outside the image", r.offset))
                })?;
                slot.copy_from_slice(&v.to_le_bytes());
            }
            self.modules[mi].nso.relocs = relocs;
        }
        Ok(())
    }

    /// The trampolines made (one per unresolved or HLE'd import no module defines).
    pub fn trampolines(&self) -> usize {
        self.tramps.len()
    }

    /// `image.json`'s text.
    pub fn json(&self) -> String {
        use json::V;
        let n = |v: u64| V::Int(v);
        let modules = self
            .modules
            .iter()
            .map(|p| {
                let m = &p.nso;
                let dynamic = |t| m.dynamic.get(&t).and_then(|v| v.first()).map_or(0, |&v| p.base + v);
                V::Obj(vec![
                    ("name", V::Str(m.name.clone())),
                    ("base", n(p.base)),
                    ("size", n(m.image.len() as u64)),
                    ("build_id", V::Str(m.build_id.clone())),
                    ("init", n(dynamic(DT_INIT))),
                    ("fini", n(dynamic(DT_FINI))),
                    (
                        "segments",
                        V::Arr(
                            m.segments
                                .iter()
                                .map(|s| {
                                    V::Obj(vec![
                                        ("name", V::Str(s.name.into())),
                                        ("addr", n(p.base + s.offset)),
                                        ("size", n(s.size)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("bss", V::Arr(vec![n(p.base + m.bss.0), n(p.base + m.bss.1)])),
                ])
            })
            .collect();
        let hooks = self
            .hooks
            .iter()
            .map(|(&a, c)| {
                V::Obj(vec![("addr", n(a)), ("c", V::Str(c.clone())), ("sym", V::Str(self.hook_syms[c].clone()))])
            })
            .collect();
        let top = V::Obj(vec![
            ("base", n(self.base)),
            ("end", n(self.end)),
            ("tramp_base", n(TRAMP_BASE)),
            ("modules", V::Arr(modules)),
            ("skipped", V::Arr(Vec::new())),
            ("hooks", V::Arr(hooks)),
            ("unresolved", V::Map(self.unresolved.iter().map(|(s, c)| (s.clone(), n(u64::from(*c)))).collect())),
            (
                "init_arrays",
                V::Map(
                    self.init_arrays
                        .iter()
                        .map(|(m, a)| (m.clone(), V::Arr(a.iter().map(|&v| n(v)).collect())))
                        .collect(),
                ),
            ),
            ("exports", V::Map(self.exports.iter().map(|(s, a)| (s.clone(), n(*a))).collect())),
        ]);
        let mut out = String::with_capacity(4 << 20);
        top.write(&mut out);
        out
    }

    /// `image.bin` and `image.json` written into `dir`, each only when it changed (a rebuild with the same inputs
    /// touches neither, so nothing that depends on them is redone).
    pub fn write(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).at(dir)?;
        build_core::fsx::write_if_changed(&dir.join("image.bin"), &self.mem)?;
        build_core::fsx::write_if_changed(&dir.join("image.json"), self.json().as_bytes())?;
        Ok(())
    }
}

/// The modules of an exefs and the patch files built into an image.
pub fn build_from_exefs(exefs: &Path, files: &[PatchFile], opts: &Options) -> Result<Image> {
    Image::build(game_files::nso::load_exefs(exefs)?, files, opts)
}

/// JSON written as Python's `json.dump(..., indent=0)` writes it (the old image.json, byte for byte): a newline
/// after every opening bracket and between items, `": "` after a key, every character past ASCII escaped.
mod json {
    pub(crate) enum V {
        Int(u64),
        Str(String),
        Arr(Vec<V>),
        Obj(Vec<(&'static str, V)>),
        Map(Vec<(String, V)>),
    }

    fn string(s: &str, out: &mut String) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                    let mut b = [0u16; 2];
                    for u in c.encode_utf16(&mut b) {
                        out.push_str(&format!("\\u{u:04x}"));
                    }
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }

    impl V {
        pub(crate) fn write(&self, out: &mut String) {
            let items = |out: &mut String, n: usize, open: char, close: char, f: &mut dyn FnMut(usize, &mut String)| {
                if n == 0 {
                    out.push(open);
                    out.push(close);
                    return;
                }
                out.push(open);
                for i in 0..n {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    f(i, out);
                }
                out.push('\n');
                out.push(close);
            };
            match self {
                V::Int(v) => out.push_str(&v.to_string()),
                V::Str(s) => string(s, out),
                V::Arr(a) => items(out, a.len(), '[', ']', &mut |i, o| a[i].write(o)),
                V::Obj(m) => items(out, m.len(), '{', '}', &mut |i, o| {
                    string(m[i].0, o);
                    o.push_str(": ");
                    m[i].1.write(o);
                }),
                V::Map(m) => items(out, m.len(), '{', '}', &mut |i, o| {
                    string(&m[i].0, o);
                    o.push_str(": ");
                    m[i].1.write(o);
                }),
            }
        }
    }

    #[cfg(test)]
    #[test]
    fn as_python_writes_it() {
        let v = V::Obj(vec![
            ("a", V::Arr(vec![V::Int(1), V::Int(2)])),
            ("b", V::Arr(Vec::new())),
            ("c", V::Str("é\"\u{1F600}".into())),
        ]);
        let mut s = String::new();
        v.write(&mut s);
        assert_eq!(s, "{\n\"a\": [\n1,\n2\n],\n\"b\": [],\n\"c\": \"\\u00e9\\\"\\ud83d\\ude00\"\n}");
    }
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod relocation_tests;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn module(name: &str) -> Nso {
        // (the synthetic module of game-files' tests is not public: a module of this crate's own, through its bytes)
        Nso::parse(name.into(), &synthetic()).unwrap()
    }

    /// A small module: MOD0, a dynamic section with a symbol table (an export `exp`, an import `imp` and a weak
    /// import `wk`), relocations of every kind the image takes, and an init array.
    pub(super) fn synthetic() -> Vec<u8> {
        let mut img = vec![0u8; 0x400];
        let w32 = |img: &mut [u8], at: usize, v: u32| img[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let w64 = |img: &mut [u8], at: usize, v: u64| img[at..at + 8].copy_from_slice(&v.to_le_bytes());
        w32(&mut img, 4, 0x10);
        img[0x10..0x14].copy_from_slice(b"MOD0");
        for (k, to) in [0x200i64, 0x380, 0x3c0, 0x3c0, 0x3c0].into_iter().enumerate() {
            w32(&mut img, 0x14 + 4 * k, (to - 0x10) as u32);
        }
        // symbols at 0x100: [1] exp (func, defined at 0x40), [2] imp (undefined), [3] wk (weak, undefined)
        let sym = |img: &mut [u8], i: usize, name: u32, info: u8, shndx: u16, value: u64| {
            let at = 0x100 + 24 * i;
            w32(img, at, name);
            img[at + 4] = info;
            img[at + 6..at + 8].copy_from_slice(&shndx.to_le_bytes());
            w64(img, at + 8, value);
        };
        sym(&mut img, 1, 1, 0x12, 1, 0x40);
        sym(&mut img, 2, 5, 0x12, 0, 0);
        sym(&mut img, 3, 9, 0x22, 0, 0);
        img[0x160..0x16c].copy_from_slice(b"\0exp\0imp\0wk\0");
        // RELA at 0x180: RELATIVE 0x340 (+0x44), ABS64 exp at 0x348 (+4), GLOB_DAT imp at 0x350, JUMP_SLOT wk at 0x358
        for (k, (off, info, add)) in [
            (0x340u64, 1027u64, 0x44u64),
            (0x348, (1 << 32) | 257, 4),
            (0x350, (2 << 32) | 1025, 0),
            (0x358, (3 << 32) | 1026, 0),
        ]
        .into_iter()
        .enumerate()
        {
            w64(&mut img, 0x180 + 24 * k, off);
            w64(&mut img, 0x188 + 24 * k, info);
            w64(&mut img, 0x190 + 24 * k, add);
        }
        w64(&mut img, 0x360, 0x40); // the init array's word (relocated by the RELATIVE below? no: a plain offset)
        let dynamic: [(i64, u64); 8] =
            [(6, 0x100), (5, 0x160), (10, 12), (11, 24), (7, 0x180), (8, 96), (25, 0x340), (27, 8)];
        for (i, (t, v)) in dynamic.into_iter().enumerate() {
            w64(&mut img, 0x200 + 16 * i, t as u64);
            w64(&mut img, 0x208 + 16 * i, v);
        }
        let segs = [(0usize, 0x100usize), (0x100, 0x200), (0x300, 0x80)];
        let mut raw = vec![0u8; 0x100];
        raw[..4].copy_from_slice(b"NSO0");
        let mut body = Vec::new();
        for (i, &(moff, size)) in segs.iter().enumerate() {
            raw[0x10 + 0x10 * i..0x14 + 0x10 * i].copy_from_slice(&((0x100 + body.len()) as u32).to_le_bytes());
            raw[0x14 + 0x10 * i..0x18 + 0x10 * i].copy_from_slice(&(moff as u32).to_le_bytes());
            raw[0x18 + 0x10 * i..0x1c + 0x10 * i].copy_from_slice(&(size as u32).to_le_bytes());
            raw[0x60 + 4 * i..0x64 + 4 * i].copy_from_slice(&(size as u32).to_le_bytes());
            body.extend_from_slice(&img[moff..moff + size]);
        }
        raw[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        raw.extend_from_slice(&body);
        raw
    }

    #[test]
    fn modules_are_laid_out_relocated_and_bound() {
        let opts = Options { hle: ["exp".to_string()].into(), ..Options::default() };
        let img = Image::build(vec![module("rtld"), module("main")], &[], &opts).unwrap();
        // each module: .data ends at 0x380, + 0x40 bss = 0x3c0, a page
        assert_eq!((img.base, img.end, img.modules[1].base), (CODE_39BIT, CODE_39BIT + 0x2000, CODE_39BIT + 0x1000));
        let word = |m: usize, off: u64| img.u64_at(img.modules[m].base + off).unwrap();
        assert_eq!(word(1, 0x340), CODE_39BIT + 0x1000 + 0x44, "RELATIVE: the module's base + addend");
        assert_eq!(word(1, 0x348), CODE_39BIT + 0x40 + 4, "ABS64: the first definition (rtld's) + addend");
        assert_eq!(word(1, 0x350), TRAMP_BASE, "an unresolved import: a trampoline");
        assert_eq!(word(1, 0x358), 0, "a weak import: 0");
        assert_eq!(img.unresolved, [("imp".to_string(), 2)], "referenced from both modules");
        assert_eq!(img.hooks.get(&(CODE_39BIT + 0x40)).map(String::as_str), Some("imp_exp"));
        assert_eq!(img.exports, [("exp".to_string(), CODE_39BIT + 0x40)]);
        assert_eq!(img.init_arrays[1].1, [CODE_39BIT + 0x1000 + 0x44]);
        let json = img.json();
        assert!(json.starts_with("{\n\"base\": 2147483648,\n\"end\": 2147491840,\n\"tramp_base\": 117440512,\n\"modules\": [\n{\n\"name\": \"rtld\""));
        // the lifter reads it back
        let d = std::env::temp_dir().join(format!("image-build-test-{}", std::process::id()));
        img.write(&d).unwrap();
        let back = crate::Image::load(&d).unwrap();
        assert_eq!((back.base(), back.end()), (img.base, img.end));
        assert!(back.in_code(CODE_39BIT + 0x1000) && !back.in_code(CODE_39BIT + 0x1100));
        assert_eq!(back.hooks().len(), 2);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_relocation_outside_its_own_module_refuses_the_image() {
        // (past its end, into the next module's first page; at the very end of the address space: refused, not written
        // and no overflow)
        for off in [0xffcu64, 0x1000, u64::MAX - 3] {
            let mut rtld = module("rtld");
            rtld.relocs[0].offset = off;
            let got = Image::build(vec![rtld, module("main")], &[], &Options::default());
            assert!(
                matches!(&got, Err(Error::Format { detail, .. }) if detail.contains("outside the module")),
                "{off:#x}: {got:?}"
            );
        }
        let mut far = module("main");
        let relative = far.relocs.iter().position(|r| r.kind == RelocKind::Relative).unwrap();
        far.relocs[relative].addend = i64::MIN;
        assert!(Image::build(vec![far], &[], &Options::default()).is_err(), "a relative address below 0");
    }

    #[test]
    fn hle_changes_a_weak_import_and_an_aslr_moves_everything() {
        let opts = Options { aslr: 0x5000, hle: ["wk".to_string()].into(), ..Options::default() };
        let img = Image::build(vec![module("main")], &[], &opts).unwrap();
        assert_eq!(img.base, CODE_39BIT + 0x5000);
        assert_eq!(img.u64_at(img.base + 0x358).unwrap(), TRAMP_BASE + 16, "an HLE'd weak import gets its trampoline");
        assert_eq!(c_ident("_ZN2nn2os4Wait"), "imp__ZN2nn2os4Wait");
        assert_eq!(c_ident("a.b$c"), "imp_a_b_c");
    }
}

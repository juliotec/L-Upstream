//! The LLVM IR writer: mid IR functions as one textual LLVM module (a `.ll` file) the pinned clang compiles.
//!
//! **The ABI**: every guest function is `void f_<entry>(Ctx *)` over the context of builder/rt/a64.h ([`ctx`]); a call
//! of another guest function is a C call of it, a tail call is `musttail`, a BLR or a BR that is no table goes
//! through a two-slot site cache as `A64_ICALL` / `A64_IJUMP` do (rt/a64.h), and the runtime's functions are rt's.
//!
//! **Guest state** lives in a local slot per location (an `alloca`, which LLVM turns into SSA values): loaded from the
//! context at entry, stored back before anything outside the function can look (a call, an SVC, a trap, an exit), and
//! loaded again after a call. Only the locations the function writes are stored back. The context parameter is
//! `noalias`: no guest memory access can touch it, so a guest store never forces a register back to memory.
//!
//! **Fast paths** ([`Op::Checked`]): the fast value is computed inline, and a branch weighted unlikely goes around a
//! call of the helper, whose value a phi takes. Each helper is written once per module, `internal`, `noinline` and
//! `cold`, reading no memory: LLVM keeps the slow path out of the hot code and lays it apart.

use mir::{
    BinOp, BlockId, Callee, Dest, Edge, FBinOp, FCast, FClass, FPred, FUnOp, Flag, Function, Global, HostArg, Loc, Op,
    Order, Pred, Reduce, Rmw, Space, Table, Term, Type, UnOp, Value,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

mod sync;

/// The context's layout (builder/rt/a64.h's `GuestContext`, which asserts it): byte offsets of its fields.
pub mod ctx {
    /// `x[0]`; `x[n]` is at `8 * n`, and `x[31]` is SP.
    pub const X: u64 = 0;
    /// `x[31]`.
    pub const SP: u64 = 31 * 8;
    /// `n`, `z`, `c`, `v`: one byte each, from here.
    pub const NZCV: u64 = 264;
    /// `vreg[0]`; `vreg[n]`, V`n`'s 16 bytes (its low byte first), is at `VREG + 16 * n`.
    pub const VREG: u64 = 272;
    /// `tpidr_el0`.
    pub const TPIDR: u64 = 784;
    /// `tpidrro_el0`.
    pub const TPIDRRO: u64 = 792;
    /// `fpcr`.
    pub const FPCR: u64 = 800;
    /// `fpsr`.
    pub const FPSR: u64 = 808;
    /// `poll`, a `uint32_t`.
    pub const POLL: u64 = 856;
    /// `piece`, a `uint32_t`: which entry block the next piece of a split function starts at.
    pub const PIECE: u64 = 860;
    /// `sizeof(Ctx)`.
    pub const SIZE: u64 = 864;
}

/// What the build asks of the lifted code, decided when the IR is written (gen/rt/a64_config.h tells the host).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Back edges poll for an interrupt (A64_POLLING).
    pub polls: bool,
    /// A store marks its pages only where the GPU bitmap says a GPU mapping is (A64_MARK_BITMAP).
    pub mark_bitmap: bool,
    /// The soft write watch's pages (A64_DIRTY_PAGES).
    pub dirty_pages: u64,
    /// Where guest address 0 is in the host (A64_HOST_BASE).
    pub memory_base: u64,
    /// A guest call gives back X19 to X29 and SP as they were (the calling convention): they are not reloaded after
    /// it (sync.rs).
    pub callee_saved: bool,
    /// Each fast path ([`Op::Checked`]) counts its runs into the external `i64` `lifter_fast_checks`, and those that
    /// took the slow path into `lifter_slow_checks` (for tests/test_lifter_semantics.py: both paths run).
    pub count_checks: bool,
}

impl Default for Options {
    /// The native build's: polls, the GPU bitmap, 2^27 pages, the window at 16 TB, callee-saved registers kept.
    fn default() -> Options {
        Options {
            polls: true,
            mark_bitmap: true,
            dirty_pages: 1 << 27,
            memory_base: 0x1000_0000_0000,
            callee_saved: true,
            count_checks: false,
        }
    }
}

/// A module being written: functions are added one by one, and [`Module::finish`] gives the text.
pub struct Module {
    opts: Options,
    body: String,
    globals: String,
    defined: BTreeSet<u64>,
    called: BTreeSet<u64>,
    runtime: BTreeSet<&'static str>,
    intrinsics: BTreeSet<String>,
    tables: BTreeSet<Table>,
    /// An instrumented unit's name (`--profile-generate`): its blocks and branches count into `@lsp.counts`.
    unit: Option<String>,
    /// The counters given out so far, and what each counts.
    counted: Vec<profile::Counted>,
    counters: u32,
    /// Metadata nodes after `!0` (branch weights, entry counts), `!1` on.
    meta: Vec<String>,
    /// The helpers written so far, by name.
    helpers: BTreeSet<String>,
    /// The node of an unlikely branch's weights (`!{1, 2000}`), once one is needed.
    unlikely: Option<String>,
    /// Every symbol a function of the module is defined as (`f_<entry>`, or a body's own).
    symbols: BTreeSet<String>,
    /// The host's functions the IR calls or takes the address of, by name: their declarations.
    host_functions: BTreeMap<String, String>,
    /// The host's globals the IR reads or writes, by name: their declarations.
    host_globals: BTreeMap<String, String>,
    /// The module's `source_filename`, when given.
    source_filename: Option<String>,
}

/// What the writer knows of a function's runs: its code's hash (the profile's key), its counts from a profile, and
/// whether it never ran.
#[derive(Clone, Copy, Debug, Default)]
pub struct Runs<'p> {
    /// Its instruction words' [`profile::code_hash`].
    pub hash: u64,
    /// Its counts, when a profile has them for this code.
    pub counts: Option<&'p profile::Function>,
    /// Whether the profile says it never ran (it is then `cold`: laid out apart from the code that runs; the lift puts
    /// it in a cold unit, which the compile builds at its own level).
    pub cold: bool,
}

/// The runtime's functions the IR may call, with their declarations.
const RUNTIME: &[(&str, &str)] = &[
    ("a64_svc", "declare void @a64_svc(ptr nocapture, i32)"),
    ("a64_trap", "declare void @a64_trap(ptr nocapture, i64, i32)"),
    ("a64_icall_site", "declare void @a64_icall_site(ptr nocapture, i64, ptr)"),
    ("a64_ijump_site", "declare void @a64_ijump_site(ptr nocapture, i64, ptr)"),
    ("a64_lookup", "declare ptr @a64_lookup(i64)"),
    ("a64_poll_cur", "declare void @a64_poll_cur()"),
    ("recomp_ldxr", "declare i64 @recomp_ldxr(ptr nocapture, i64, i32)"),
    ("recomp_stxr", "declare i32 @recomp_stxr(ptr nocapture, i64, i32, i64)"),
    ("recomp_ldxp", "declare void @recomp_ldxp(ptr nocapture, i64, i32, ptr nocapture, ptr nocapture)"),
    ("recomp_stxp", "declare i32 @recomp_stxp(ptr nocapture, i64, i32, i64, i64)"),
    ("recomp_clrex", "declare void @recomp_clrex(ptr nocapture)"),
    ("recomp_cntpct", "declare i64 @recomp_cntpct(ptr nocapture)"),
    // (host/hos_setjmp.c keeps the context's address, to find its slots by: compared, never read through. So no
    // `nocapture`; and not `captures(address)` either, which LLVM reads only from 21 on, where CI's clang is 18)
    ("recomp_sj_push", "declare ptr @recomp_sj_push(ptr, i64, i64, i64, i64)"),
    ("recomp_sj_setjmp", "declare i32 @recomp_sj_setjmp(ptr) returns_twice"),
    ("recomp_sj_target", "declare i64 @recomp_sj_target(ptr)"),
    ("recomp_sj_pop", "declare void @recomp_sj_pop(ptr)"),
    ("a64_icall", "declare void @a64_icall(ptr nocapture, i64)"),
];

impl Module {
    /// An empty module.
    pub fn new(opts: Options) -> Module {
        Module {
            opts,
            body: String::new(),
            globals: String::new(),
            defined: BTreeSet::new(),
            called: BTreeSet::new(),
            runtime: BTreeSet::new(),
            intrinsics: BTreeSet::new(),
            tables: BTreeSet::new(),
            unit: None,
            counted: Vec::new(),
            counters: 0,
            meta: Vec::new(),
            helpers: BTreeSet::new(),
            unlikely: None,
            symbols: BTreeSet::new(),
            host_functions: BTreeMap::new(),
            host_globals: BTreeMap::new(),
            source_filename: None,
        }
    }

    /// The module's `source_filename` (the object's file symbol): its file's name, so the object is the same whether
    /// the compiler reads the file or the text on its standard input.
    pub fn set_source_filename(&mut self, name: &str) {
        self.source_filename = Some(name.to_string());
    }

    /// A module whose functions count their blocks' entries and their branches' taken edges, for a profile: `unit` is
    /// its name in the profile's map (host/guest_profile.c gets the counters by it).
    pub fn instrumented(opts: Options, unit: &str) -> Module {
        Module { unit: Some(unit.to_string()), ..Module::new(opts) }
    }

    /// The counters of an instrumented module's functions so far, for the profile's map.
    pub fn counted(&self) -> &[profile::Counted] {
        &self.counted
    }

    /// `f` added, with what a profile says of its runs. `f` must verify (Function::verify).
    pub fn add(&mut self, f: &Function, runs: Runs<'_>) {
        if f.symbol().is_none() {
            self.defined.insert(f.entry);
        }
        self.symbols.insert(f.name());
        for (_, h) in f.helpers() {
            if self.helpers.insert(h.name.clone()) {
                let text = FnWriter::new(self, &h.body, Runs::default()).write_helper(h);
                self.body.push_str(&text);
            }
        }
        let text = FnWriter::new(self, f, runs).write();
        self.body.push_str(&text);
    }

    /// A metadata node's reference (`!N`), the node added.
    fn node(&mut self, text: String) -> String {
        self.meta.push(text);
        format!("!{}", self.meta.len())
    }

    /// The weights of a branch whose first edge is unlikely, as a node reference.
    fn unlikely(&mut self) -> String {
        if let Some(n) = &self.unlikely {
            return n.clone();
        }
        let n = self.node("!{!\"branch_weights\", i32 1, i32 2000}".into());
        self.unlikely = Some(n.clone());
        n
    }

    /// How many functions it holds.
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// The host function `name` declared as `decl` (its first declaration stays: one name, one signature).
    fn host_function(&mut self, name: &str, decl: String) {
        self.host_functions.entry(name.to_string()).or_insert(decl);
    }

    /// The host global `g` declared, of type `ty` (an LLVM type).
    fn host_global(&mut self, g: &Global, ty: &str) {
        let tls = if g.thread_local { "thread_local " } else { "" };
        self.host_globals
            .entry(g.symbol.clone())
            .or_insert_with(|| format!("@{} = external {tls}global {ty}", g.symbol));
    }

    /// The bytes of its functions' and globals' text so far (the declarations and metadata [`Module::finish`] adds
    /// are left out): a unit's size, and by difference a function's.
    pub fn size(&self) -> usize {
        self.body.len() + self.globals.len()
    }

    /// The module's text.
    pub fn finish(self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "; Written by longshot-builder (lifter/): {} functions of the game's code. Do not edit.",
            self.len()
        );
        if let Some(name) = &self.source_filename {
            let _ = writeln!(out, "source_filename = \"{}\"", name.replace(['\\', '"'], "_"));
        }
        let _ = writeln!(out, "@a64_dirty = external global [{} x i8]", self.opts.dirty_pages);
        if self.opts.mark_bitmap {
            let _ = writeln!(out, "@a64_gpu_pages = external global [{} x i8]", self.opts.dirty_pages / 8);
        }
        if self.opts.count_checks {
            out.push_str("@lifter_fast_checks = external global i64\n@lifter_slow_checks = external global i64\n");
        }
        out.push_str(&self.globals);
        for t in &self.tables {
            let bytes: String = t.bytes().iter().map(|b| format!("\\{b:02X}")).collect();
            let _ =
                writeln!(out, "{} = private unnamed_addr constant [{} x i8] c\"{bytes}\"", table_name(*t), t.entries());
        }
        for (name, decl) in RUNTIME {
            if self.runtime.contains(name) {
                let _ = writeln!(out, "{decl}");
            }
        }
        for i in &self.intrinsics {
            let _ = writeln!(out, "{i}");
        }
        for t in self.called.difference(&self.defined) {
            let _ = writeln!(out, "declare void @f_{t:08x}(ptr nocapture)");
        }
        for decl in self.host_globals.values() {
            let _ = writeln!(out, "{decl}");
        }
        for (name, decl) in &self.host_functions {
            if !self.symbols.contains(name) {
                let _ = writeln!(out, "{decl}");
            }
        }
        if let Some(unit) = &self.unit {
            // (the counters, registered with host/guest_profile.c when the game starts)
            let n = self.counters.max(1);
            let _ = writeln!(out, "@lsp.counts = internal global [{n} x i64] zeroinitializer, align 8");
            let _ =
                writeln!(out, "@lsp.unit = private unnamed_addr constant [{} x i8] c\"{unit}\\00\"", unit.len() + 1);
            out.push_str("@llvm.global_ctors = appending global [1 x { i32, ptr, ptr }] [{ i32, ptr, ptr } { i32 65535, ptr @lsp.ctor, ptr null }]\n");
            out.push_str("declare void @lsp_register(ptr, i64, ptr)\n");
            let _ = writeln!(
                out,
                "define internal void @lsp.ctor() {{\n  call void @lsp_register(ptr @lsp.counts, i64 {n}, ptr @lsp.unit)\n  ret void\n}}"
            );
        }
        out.push_str(&self.body);
        out.push_str("attributes #0 = { nounwind uwtable }\n");
        out.push_str("attributes #1 = { nounwind uwtable cold }\n");
        out.push_str("attributes #2 = { nounwind willreturn noinline cold optsize memory(none) }\n");
        out.push_str("attributes #3 = { nounwind willreturn noinline cold optsize memory(read) }\n");
        out.push_str("attributes #4 = { noinline }\n");
        out.push_str("!0 = !{!\"branch_weights\", i32 2000, i32 1}\n");
        for (i, m) in self.meta.iter().enumerate() {
            let _ = writeln!(out, "!{} = {m}", i + 1);
        }
        out
    }
}

fn llvm_type(ty: Type) -> String {
    match ty {
        Type::I1 => "i1".into(),
        Type::I8 => "i8".into(),
        Type::I16 => "i16".into(),
        Type::I32 => "i32".into(),
        Type::I64 => "i64".into(),
        Type::I128 => "i128".into(),
        Type::F16 => "half".into(),
        Type::F32 => "float".into(),
        Type::F64 => "double".into(),
        Type::Vec(n, e) => format!("<{n} x {}>", llvm_type(e.scalar())),
    }
}

/// A type as an intrinsic's name spells it: i32, f64, v4f32.
fn mangle(ty: Type) -> String {
    let scalar = |t: Type| if t.is_float() { format!("f{}", t.bits()) } else { format!("i{}", t.bits()) };
    match ty {
        Type::Vec(n, e) => format!("v{n}{}", scalar(e.scalar())),
        t => scalar(t),
    }
}

/// The mask of `llvm.is.fpclass`'s classes that are subnormal (negative and positive).
const FPCLASS_SUBNORMAL: u32 = 0x10 | 0x80;

/// The mask of `llvm.is.fpclass`'s NaN classes (signalling and quiet).
const FPCLASS_NAN: u32 = 0x1 | 0x2;

/// The global a table is written as.
fn table_name(t: Table) -> &'static str {
    match t {
        Table::AesSbox => "@lifter.aes_sbox",
        Table::AesInvSbox => "@lifter.aes_inv_sbox",
        Table::RecipSqrt => "@lifter.recip_sqrt",
    }
}

fn loc_name(loc: Loc) -> String {
    format!("%r.{loc}")
}

fn loc_offset(loc: Loc) -> u64 {
    match loc {
        Loc::X(n) => ctx::X + 8 * u64::from(n),
        Loc::Sp => ctx::SP,
        Loc::Flag(f) => {
            ctx::NZCV
                + match f {
                    Flag::N => 0,
                    Flag::Z => 1,
                    Flag::C => 2,
                    Flag::V => 3,
                }
        }
        Loc::Tpidr => ctx::TPIDR,
        Loc::TpidrRo => ctx::TPIDRRO,
        Loc::Fpcr => ctx::FPCR,
        Loc::Fpsr => ctx::FPSR,
        Loc::V(n) => ctx::VREG + 16 * u64::from(n),
    }
}

fn order_name(o: Order) -> &'static str {
    match o {
        Order::Acquire => "acquire",
        Order::Release => "release",
        Order::SeqCst => "seq_cst",
    }
}

/// Whether the low `w` bits of `bits` are a NaN of that width's float format.
fn is_nan_bits(w: u32, bits: u128) -> bool {
    let frac = match w {
        16 => 10,
        32 => 23,
        _ => 52,
    };
    let exp = (1u128 << (w - 1 - frac)) - 1;
    (bits >> frac) & exp == exp && bits & ((1u128 << frac) - 1) != 0
}

/// A constant of `ty` with these bits, as an LLVM literal: `-1`, `true`, `0x3FF0000000000000` (a float, written as
/// the double of its value), `0xH3C00` (a half), `<i32 1, i32 2>`, `zeroinitializer`.
/// The bits of lane `i` (of `w` bits) of a constant's `bits` ([`FnWriter::constant`]: a vector wider than 128 bits has
/// its low 128 bits there, which repeat)
fn lane_bits(bits: u128, i: u32, w: u32) -> u128 {
    let mask = if w >= 128 { u128::MAX } else { (1u128 << w) - 1 };
    (bits >> ((i * w) % 128)) & mask
}

fn literal(ty: Type, bits: u128) -> String {
    match ty {
        Type::Vec(n, e) => {
            if bits == 0 {
                return "zeroinitializer".into();
            }
            let et = e.scalar();
            let w = et.bits();
            let lanes: Vec<String> = (0..u32::from(n))
                .map(|i| format!("{} {}", llvm_type(et), literal(et, lane_bits(bits, i, w))))
                .collect();
            format!("<{}>", lanes.join(", "))
        }
        Type::I1 => if bits & 1 == 1 { "true" } else { "false" }.into(),
        Type::F16 => format!("0xH{:04X}", bits as u16),
        Type::F32 => format!("0x{:016X}", f64::from(f32::from_bits(bits as u32)).to_bits()),
        Type::F64 => format!("0x{:016X}", bits as u64),
        _ => {
            // (as a signed number of its width: LLVM reads a literal as that)
            let w = ty.bits();
            let signed = if w == 128 { bits as i128 } else { ((bits << (128 - w)) as i128) >> (128 - w) };
            signed.to_string()
        }
    }
}

/// One function's text.
struct FnWriter<'m, 'f> {
    m: &'m mut Module,
    f: &'f Function,
    out: String,
    tmp: u32,
    used: BTreeSet<Loc>,
    /// What it syncs where (sync.rs).
    sync: sync::Sync,
    /// The block being written (its exits spill what is dirty at its end).
    cur: usize,
    sites: u32,
    /// The blocks the edges that need code of their own go through: their label and their edge.
    trampolines: Vec<(String, Edge)>,
    /// The label of the LLVM block being written (a phi names its predecessors by it).
    at: String,
    /// What a profile says of its runs.
    runs: Runs<'f>,
    /// Its counters, when the module is instrumented.
    counters: Vec<(profile::Counter, u32)>,
}

impl<'m, 'f> FnWriter<'m, 'f> {
    fn new(m: &'m mut Module, f: &'f Function, runs: Runs<'f>) -> FnWriter<'m, 'f> {
        let mut used = BTreeSet::new();
        for (_, b) in f.blocks() {
            for &v in &b.insts {
                match f.inst(v).op {
                    Op::Get(loc) | Op::Set(loc, _) => {
                        used.insert(loc);
                    }
                    _ => {}
                }
            }
        }
        let sync = sync::sync(f, m.opts.callee_saved);
        FnWriter {
            m,
            f,
            out: String::new(),
            tmp: 0,
            used,
            sync,
            cur: 0,
            sites: 0,
            trampolines: Vec::new(),
            at: "entry".into(),
            runs,
            counters: Vec::new(),
        }
    }

    fn temp(&mut self) -> String {
        self.tmp += 1;
        format!("%t{}", self.tmp)
    }

    /// A new block label's name (without the `%`).
    fn fresh_label(&mut self) -> String {
        self.tmp += 1;
        format!("t{}", self.tmp)
    }

    fn line(&mut self, text: impl AsRef<str>) {
        self.out.push_str("  ");
        self.out.push_str(text.as_ref());
        self.out.push('\n');
    }

    fn label(&mut self, name: &str) {
        let _ = writeln!(self.out, "{name}:");
        self.at = name.to_string();
    }

    fn ty(&self, v: Value) -> Type {
        self.f.ty(v).expect("a value")
    }

    /// `v` as an operand: a constant's literal, a helper's parameter's name, else its own name.
    fn val(&self, v: Value) -> String {
        if let Op::Param { index, .. } = self.f.inst(v).op {
            return format!("%p{index}");
        }
        match self.constant(v) {
            Some(bits) => literal(self.ty(v), bits),
            None => format!("%v{}", v.0),
        }
    }

    /// The bits of `v` when it is a constant the writer prints as a literal: a constant, or a bitcast or splat of one
    /// (not a float NaN: LLVM's float literals are written as doubles, which keep no NaN's payload). A vector wider than
    /// 128 bits (a splat's: `<4 x i64>` of SQRDMULH's rounding) has its low 128 bits here, which repeat across it
    /// ([`lane_bits`]); no other constant that wide is made.
    fn constant(&self, v: Value) -> Option<u128> {
        let bits = match self.f.inst(v).op {
            Op::Const { ty, value } => {
                assert!(ty.bits() <= 128, "a constant of {ty:?}: wider than its 128 bits of value");
                value
            }
            Op::Bitcast(_, x) => self.constant(x)?,
            Op::Splat(ty, x) => {
                let e = self.constant(x)?;
                let w = ty.elem().bits();
                (0..ty.lanes()).take_while(|i| i * w < 128).fold(0u128, |acc, i| acc | (e << (i * w)))
            }
            _ => return None,
        };
        let ty = self.ty(v);
        let w = ty.elem().bits();
        let has_nan = ty.is_float() && (0..ty.lanes()).any(|i| is_nan_bits(w, lane_bits(bits, i, w)));
        (!has_nan).then_some(bits)
    }

    /// `v` with its type: `i64 %v3`.
    fn typed(&self, v: Value) -> String {
        format!("{} {}", llvm_type(self.ty(v)), self.val(v))
    }

    fn ctx_ptr(&mut self, offset: u64) -> String {
        let p = self.temp();
        self.line(format!("{p} = getelementptr inbounds i8, ptr %c, i64 {offset}"));
        p
    }

    fn guest_ptr(&mut self, addr: Value) -> String {
        let p = self.temp();
        let a = self.val(addr);
        self.line(format!("{p} = getelementptr i8, ptr %mem, i64 {a}"));
        p
    }

    fn write(mut self) -> String {
        let name = self.f.name();
        let attrs = if self.runs.cold { "#1" } else { "#0" };
        let prof = match self.runs.counts {
            Some(c) => format!(" !prof {}", self.m.node(format!("!{{!\"function_entry_count\", i64 {}}}", c.entries))),
            None => String::new(),
        };
        let linkage = if self.f.pieces.as_ref().is_some_and(|p| p.index > 0) { "internal " } else { "" };
        let _ = writeln!(self.out, "define {linkage}void @{name}(ptr noalias nocapture %c) {attrs}{prof} {{");
        self.label("entry");
        for loc in self.used.clone() {
            self.line(format!("{} = alloca {}", loc_name(loc), llvm_type(loc.ty())));
        }
        // (an LDXP's two halves, which the runtime writes)
        if self.f.blocks().any(|(_, b)| b.insts.iter().any(|&v| matches!(self.f.inst(v).op, Op::Ldxp { .. }))) {
            self.line("%x.lo = alloca i64");
            self.line("%x.hi = alloca i64");
        }
        // (guest memory's window, named once: LLVM folds it back to the constant)
        let base = self.m.opts.memory_base;
        self.line(format!("%mem = inttoptr i64 {base} to ptr"));
        let entry = self.sync.entry.clone();
        self.reload(&entry);
        if self.f.entries.is_empty() {
            self.line(format!("br label %b{}", self.f.start.0));
        } else {
            // (a piece entered by another: the entry block it names)
            let p = self.ctx_ptr(ctx::PIECE);
            let k = self.temp();
            self.line(format!("{k} = load i32, ptr {p}, align 4"));
            let arms: String =
                self.f.entries.iter().enumerate().map(|(i, b)| format!(" i32 {i}, label %b{}", b.0)).collect();
            self.line(format!("switch i32 {k}, label %b{} [{arms} ]", self.f.start.0));
        }
        for (id, block) in self.f.blocks() {
            self.cur = id.0 as usize;
            self.label(&format!("b{}", id.0));
            if block.guest_start {
                self.count(profile::Counter::Block(block.pc), None);
            }
            for &v in &block.insts {
                self.inst(v);
            }
            let term = block.term.clone().expect("a verified function");
            self.term(&term);
            for (label, edge) in std::mem::take(&mut self.trampolines) {
                self.label(&label);
                self.edge_code(edge);
            }
        }
        self.out.push_str("}\n\n");
        if self.m.unit.is_some() && !self.f.wrapper {
            let counters = std::mem::take(&mut self.counters);
            // (the pieces of a split function count as the function: their counters one list)
            match self.m.counted.last_mut() {
                Some(last) if self.f.pieces.is_some() && last.entry == self.f.entry => last.counters.extend(counters),
                _ => self.m.counted.push(profile::Counted { entry: self.f.entry, hash: self.runs.hash, counters }),
            }
        }
        self.out
    }

    /// A helper's definition: its parameters `%p0` on, its body, and the return of its result.
    fn write_helper(mut self, h: &mir::Helper) -> String {
        let params: Vec<String> =
            h.params.iter().enumerate().map(|(i, &t)| format!("{} %p{i}", llvm_type(t))).collect();
        let reads = h.body.blocks().any(|(_, b)| b.insts.iter().any(|&v| matches!(h.body.inst(v).op, Op::Lookup(..))));
        let attrs = if reads { "#3" } else { "#2" };
        let ret = llvm_type(h.ret());
        let _ = writeln!(self.out, "define internal {ret} @{}({}) {attrs} {{", h.name, params.join(", "));
        self.label("entry");
        for &v in &self.f.block(self.f.start).insts {
            self.inst(v);
        }
        let r = self.val(h.result);
        self.line(format!("ret {ret} {r}"));
        self.out.push_str("}\n\n");
        self.out
    }

    /// The context's values of `locs` into their slots (at entry and after a call).
    fn reload(&mut self, locs: &[Loc]) {
        for &loc in locs {
            let p = self.ctx_ptr(loc_offset(loc));
            let t = self.temp();
            match loc.ty() {
                Type::I1 => {
                    let b = self.temp();
                    self.line(format!("{b} = load i8, ptr {p}, align 1"));
                    self.line(format!("{t} = icmp ne i8 {b}, 0"));
                }
                ty => self.line(format!("{t} = load {0}, ptr {p}, align 8", llvm_type(ty))),
            }
            self.line(format!("store {} {t}, ptr {}", llvm_type(loc.ty()), loc_name(loc)));
        }
    }

    /// The slots of `locs` back into the context (before a call or an exit).
    fn spill(&mut self, locs: &[Loc]) {
        for &loc in locs {
            let t = self.temp();
            self.line(format!("{t} = load {}, ptr {}", llvm_type(loc.ty()), loc_name(loc)));
            let p = self.ctx_ptr(loc_offset(loc));
            match loc.ty() {
                Type::I1 => {
                    let b = self.temp();
                    self.line(format!("{b} = zext i1 {t} to i8"));
                    self.line(format!("store i8 {b}, ptr {p}, align 1"));
                }
                ty => self.line(format!("store {} {t}, ptr {p}, align 8", llvm_type(ty))),
            }
        }
    }

    /// What is spilled before and reloaded after the call or SVC `v`.
    fn around(&self, v: Value) -> (Vec<Loc>, Vec<Loc>) {
        self.sync.around.get(&v).cloned().expect("every call and SVC is synced")
    }

    /// The current block's dirty slots into the context: the state leaves the function here.
    fn spill_exit(&mut self) {
        let locs = self.sync.exit[self.cur].clone();
        self.spill(&locs);
    }

    /// In an instrumented module, a counter for `what` (a block's entries, or by `cond` a branch's taken edge).
    fn count(&mut self, what: profile::Counter, cond: Option<&str>) {
        if self.m.unit.is_none() || self.f.wrapper {
            return;
        }
        let i = self.m.counters;
        self.m.counters += 1;
        self.counters.push((what, i));
        let (p, old, new) = (self.temp(), self.temp(), self.temp());
        self.line(format!("{p} = getelementptr inbounds i64, ptr @lsp.counts, i64 {i}"));
        self.line(format!("{old} = load i64, ptr {p}, align 8"));
        let step = match cond {
            Some(c) => {
                let z = self.temp();
                self.line(format!("{z} = zext i1 {c} to i64"));
                z
            }
            None => "1".to_string(),
        };
        self.line(format!("{new} = add i64 {old}, {step}"));
        self.line(format!("store i64 {new}, ptr {p}, align 8"));
    }

    /// A conditional branch's weights from the profile (its block's entries, its taken count), as `, !prof !N`.
    fn weights(&mut self, pc: u64) -> String {
        let Some(c) = self.runs.counts else { return String::new() };
        let (Some(&n), Some(&t)) = (c.blocks.get(&pc), c.taken.get(&pc)) else { return String::new() };
        // (taken at most the block's entries, as a profile read says, profile::Function::clamp_taken: a profile made in
        // code may not be)
        let t = t.min(n);
        let not = n - t;
        // (LLVM's weights are 32 bits: both scaled alike to fit, their sum, n, too)
        let scale = (n.max(1) >> 31) + 1;
        let node = self.m.node(format!("!{{!\"branch_weights\", i32 {}, i32 {}}}", t / scale, not / scale));
        format!(", !prof {node}")
    }

    fn call_guest(&mut self, target: u64) {
        self.m.called.insert(target);
        self.line(format!("call void @f_{target:08x}(ptr %c)"));
    }

    /// A call of the overloaded intrinsic `llvm.<name>.<mangled types>` returning `ret`, declared once; `args` are
    /// (type, operand) pairs.
    fn call_intrinsic(&mut self, dst: &str, name: &str, ret: Type, overloads: &[Type], args: &[(String, String)]) {
        let mut full = format!("@llvm.{name}");
        for t in overloads {
            full.push('.');
            full.push_str(&mangle(*t));
        }
        let params: Vec<&str> = args.iter().map(|(t, _)| t.as_str()).collect();
        let r = llvm_type(ret);
        self.m.intrinsics.insert(format!("declare {r} {full}({})", params.join(", ")));
        let operands: Vec<String> = args.iter().map(|(t, v)| format!("{t} {v}")).collect();
        self.line(format!("{dst} = call {r} {full}({})", operands.join(", ")));
    }

    /// `v` as an intrinsic's argument: its type and its operand.
    fn arg(&self, v: Value) -> (String, String) {
        (llvm_type(self.ty(v)), self.val(v))
    }

    fn inst(&mut self, v: Value) {
        let inst = self.f.inst(v).clone();
        let dst = format!("%v{}", v.0);
        match inst.op {
            // (a constant, or a bitcast or splat of one: written where it is used)
            Op::Const { .. } => {}
            _ if self.constant(v).is_some() => {}
            Op::Get(loc) => {
                self.line(format!("{dst} = load {}, ptr {}", llvm_type(loc.ty()), loc_name(loc)));
            }
            Op::Set(loc, x) => {
                let x = self.typed(x);
                self.line(format!("store {x}, ptr {}", loc_name(loc)));
            }
            Op::Bin(op, a, b) => {
                let intrinsic = match op {
                    BinOp::SAddSat => Some("sadd.sat"),
                    BinOp::UAddSat => Some("uadd.sat"),
                    BinOp::SSubSat => Some("ssub.sat"),
                    BinOp::USubSat => Some("usub.sat"),
                    BinOp::SMin => Some("smin"),
                    BinOp::SMax => Some("smax"),
                    BinOp::UMin => Some("umin"),
                    BinOp::UMax => Some("umax"),
                    _ => None,
                };
                if let Some(name) = intrinsic {
                    let ty = self.ty(a);
                    let args = [self.arg(a), self.arg(b)];
                    self.call_intrinsic(&dst, name, ty, &[ty], &args);
                    return;
                }
                let name = match op {
                    BinOp::Add => "add",
                    BinOp::Sub => "sub",
                    BinOp::Mul => "mul",
                    BinOp::UDiv => "udiv",
                    BinOp::SDiv => "sdiv",
                    BinOp::And => "and",
                    BinOp::Or => "or",
                    BinOp::Xor => "xor",
                    BinOp::Shl => "shl",
                    BinOp::LShr => "lshr",
                    _ => "ashr",
                };
                let (a, b) = (self.typed(a), self.val(b));
                self.line(format!("{dst} = {name} {a}, {b}"));
            }
            Op::Un(op, a) => {
                let ty = self.ty(a);
                let t = llvm_type(ty);
                let x = self.val(a);
                let (ones, zero) = if ty.is_vector() {
                    (format!("splat ({} -1)", llvm_type(ty.elem())), "zeroinitializer".to_string())
                } else {
                    ("-1".to_string(), "0".to_string())
                };
                match op {
                    UnOp::Not => self.line(format!("{dst} = xor {t} {x}, {ones}")),
                    UnOp::Neg => self.line(format!("{dst} = sub {t} {zero}, {x}")),
                    UnOp::Ctlz => {
                        let args = [self.arg(a), ("i1".to_string(), "false".to_string())];
                        self.call_intrinsic(&dst, "ctlz", ty, &[ty], &args);
                    }
                    UnOp::Ctpop | UnOp::BitReverse | UnOp::ByteSwap => {
                        let name = match op {
                            UnOp::Ctpop => "ctpop",
                            UnOp::BitReverse => "bitreverse",
                            _ => "bswap",
                        };
                        let args = [self.arg(a)];
                        self.call_intrinsic(&dst, name, ty, &[ty], &args);
                    }
                }
            }
            Op::Cmp(p, a, b) => {
                let pred = match p {
                    Pred::Eq => "eq",
                    Pred::Ne => "ne",
                    Pred::Ult => "ult",
                    Pred::Ule => "ule",
                    Pred::Ugt => "ugt",
                    Pred::Uge => "uge",
                    Pred::Slt => "slt",
                    Pred::Sle => "sle",
                    Pred::Sgt => "sgt",
                    Pred::Sge => "sge",
                };
                let (a, b) = (self.typed(a), self.val(b));
                self.line(format!("{dst} = icmp {pred} {a}, {b}"));
            }
            Op::Select(c, a, b) => {
                let (c, a, b) = (self.typed(c), self.typed(a), self.typed(b));
                self.line(format!("{dst} = select {c}, {a}, {b}"));
            }
            Op::Cast(cast, to, a) => {
                let name = match cast {
                    mir::Cast::Zext => "zext",
                    mir::Cast::Sext => "sext",
                    mir::Cast::Trunc => "trunc",
                };
                let a = self.typed(a);
                self.line(format!("{dst} = {name} {a} to {}", llvm_type(to)));
            }
            Op::Bitcast(to, a) => {
                let a = self.typed(a);
                self.line(format!("{dst} = bitcast {a} to {}", llvm_type(to)));
            }
            Op::FBin(op, a, b) => {
                let name = match op {
                    FBinOp::Add => "fadd",
                    FBinOp::Sub => "fsub",
                    FBinOp::Mul => "fmul",
                    FBinOp::Div => "fdiv",
                    FBinOp::Maximum | FBinOp::Minimum => {
                        let name = if op == FBinOp::Maximum { "maximum" } else { "minimum" };
                        let ty = self.ty(a);
                        let args = [self.arg(a), self.arg(b)];
                        self.call_intrinsic(&dst, name, ty, &[ty], &args);
                        return;
                    }
                };
                let (a, b) = (self.typed(a), self.val(b));
                self.line(format!("{dst} = {name} {a}, {b}"));
            }
            Op::FUn(op, a) => {
                let name = match op {
                    FUnOp::Sqrt => "sqrt",
                    FUnOp::RoundEven => "roundeven",
                    FUnOp::Round => "round",
                    FUnOp::Floor => "floor",
                    FUnOp::Ceil => "ceil",
                    FUnOp::Trunc => "trunc",
                };
                let ty = self.ty(a);
                let args = [self.arg(a)];
                self.call_intrinsic(&dst, name, ty, &[ty], &args);
            }
            Op::Fma(a, b, c) => {
                let ty = self.ty(a);
                let args = [self.arg(a), self.arg(b), self.arg(c)];
                self.call_intrinsic(&dst, "fma", ty, &[ty], &args);
            }
            Op::FCmp(p, a, b) => {
                let pred = match p {
                    FPred::Oeq => "oeq",
                    FPred::Ogt => "ogt",
                    FPred::Oge => "oge",
                    FPred::Olt => "olt",
                    FPred::Ole => "ole",
                    FPred::Uno => "uno",
                };
                let (a, b) = (self.typed(a), self.val(b));
                self.line(format!("{dst} = fcmp {pred} {a}, {b}"));
            }
            Op::FCast(cast, to, a) => {
                let from = self.ty(a);
                let name = match cast {
                    FCast::Ext => "fpext",
                    FCast::Trunc => "fptrunc",
                    FCast::FromSigned => "sitofp",
                    FCast::FromUnsigned => "uitofp",
                    FCast::ToSigned | FCast::ToUnsigned => {
                        let name = if cast == FCast::ToSigned { "fptosi.sat" } else { "fptoui.sat" };
                        let args = [self.arg(a)];
                        self.call_intrinsic(&dst, name, to, &[to, from], &args);
                        return;
                    }
                };
                let a = self.typed(a);
                self.line(format!("{dst} = {name} {a} to {}", llvm_type(to)));
            }
            Op::Extract(a, lane) => {
                let a = self.typed(a);
                self.line(format!("{dst} = extractelement {a}, i32 {lane}"));
            }
            Op::ExtractAt(a, i) => {
                let (a, i) = (self.typed(a), self.typed(i));
                self.line(format!("{dst} = extractelement {a}, {i}"));
            }
            Op::Insert(a, lane, x) => {
                let (a, x) = (self.typed(a), self.typed(x));
                self.line(format!("{dst} = insertelement {a}, {x}, i32 {lane}"));
            }
            Op::Shuffle(a, b, mask) => {
                let (a, b) = (self.typed(a), self.typed(b));
                let lanes: Vec<String> = mask.iter().map(|m| format!("i32 {m}")).collect();
                let mask = format!("<{} x i32> <{}>", mask.len(), lanes.join(", "));
                self.line(format!("{dst} = shufflevector {a}, {b}, {mask}"));
            }
            Op::Splat(ty, x) => {
                let t = llvm_type(ty);
                let one = self.temp();
                let x = self.typed(x);
                self.line(format!("{one} = insertelement {t} poison, {x}, i32 0"));
                let n = ty.lanes();
                self.line(format!("{dst} = shufflevector {t} {one}, {t} poison, <{n} x i32> zeroinitializer"));
            }
            Op::Reduce(op, a) => {
                let name = match op {
                    Reduce::Add => "vector.reduce.add",
                    Reduce::Or => "vector.reduce.or",
                    Reduce::SMax => "vector.reduce.smax",
                    Reduce::SMin => "vector.reduce.smin",
                    Reduce::UMax => "vector.reduce.umax",
                    Reduce::UMin => "vector.reduce.umin",
                };
                let ty = self.ty(a);
                let args = [self.arg(a)];
                self.call_intrinsic(&dst, name, ty.elem(), &[ty], &args);
            }
            Op::Lookup(table, index) => {
                self.m.tables.insert(table);
                let i = if self.ty(index) == Type::I64 {
                    self.val(index)
                } else {
                    let t = self.temp();
                    let x = self.typed(index);
                    self.line(format!("{t} = zext {x} to i64"));
                    t
                };
                let p = self.temp();
                let n = table.entries();
                self.line(format!(
                    "{p} = getelementptr inbounds [{n} x i8], ptr {}, i64 0, i64 {i}",
                    table_name(table)
                ));
                self.line(format!("{dst} = load i8, ptr {p}, align 1"));
            }
            Op::Load { ty, addr, order } => {
                let p = self.guest_ptr(addr);
                let t = llvm_type(ty);
                match order {
                    None => self.line(format!("{dst} = load {t}, ptr {p}, align 1")),
                    Some(o) => {
                        let align = ty.bits() / 8;
                        self.line(format!("{dst} = load atomic {t}, ptr {p} {}, align {align}", order_name(o)));
                    }
                }
            }
            Op::Store { addr, value, order, space: Space::Guest | Space::Stack } => {
                let p = self.guest_ptr(addr);
                let x = self.typed(value);
                match order {
                    None => self.line(format!("store {x}, ptr {p}, align 1")),
                    Some(o) => {
                        let align = self.ty(value).bits() / 8;
                        self.line(format!("store atomic {x}, ptr {p} {}, align {align}", order_name(o)));
                    }
                }
            }
            Op::Mark { addr, bytes } => self.mark(addr, bytes),
            Op::AtomicRmw { op, addr, value, order } => {
                let name = match op {
                    Rmw::Xchg => "xchg",
                    Rmw::Add => "add",
                    Rmw::And => "and",
                    Rmw::Or => "or",
                    Rmw::Xor => "xor",
                    Rmw::Max => "max",
                    Rmw::Min => "min",
                    Rmw::UMax => "umax",
                    Rmw::UMin => "umin",
                };
                let p = self.guest_ptr(addr);
                let align = self.ty(value).bits() / 8;
                let x = self.typed(value);
                self.line(format!("{dst} = atomicrmw {name} ptr {p}, {x} {}, align {align}", order_name(order)));
            }
            Op::CmpXchg { addr, expected, new } => {
                let p = self.guest_ptr(addr);
                let ty = self.ty(expected);
                let pair = self.temp();
                let (e, n) = (self.typed(expected), self.typed(new));
                let align = ty.bits() / 8;
                self.line(format!("{pair} = cmpxchg ptr {p}, {e}, {n} seq_cst seq_cst, align {align}"));
                self.line(format!("{dst} = extractvalue {{ {}, i1 }} {pair}, 0", llvm_type(ty)));
            }
            Op::Fence => self.line("fence seq_cst"),
            Op::Ldxr { addr, bytes } => {
                self.m.runtime.insert("recomp_ldxr");
                let a = self.val(addr);
                self.line(format!("{dst} = call i64 @recomp_ldxr(ptr %c, i64 {a}, i32 {bytes})"));
            }
            Op::Stxr { addr, bytes, value } => {
                self.m.runtime.insert("recomp_stxr");
                let (a, x) = (self.val(addr), self.val(value));
                self.line(format!("{dst} = call i32 @recomp_stxr(ptr %c, i64 {a}, i32 {bytes}, i64 {x})"));
            }
            Op::Ldxp { addr, bytes } => {
                self.m.runtime.insert("recomp_ldxp");
                let a = self.val(addr);
                self.line(format!("call void @recomp_ldxp(ptr %c, i64 {a}, i32 {bytes}, ptr %x.lo, ptr %x.hi)"));
                let (lo, hi) = (self.temp(), self.temp());
                self.line(format!("{lo} = load i64, ptr %x.lo"));
                self.line(format!("{hi} = load i64, ptr %x.hi"));
                let (wl, wh, sh) = (self.temp(), self.temp(), self.temp());
                self.line(format!("{wl} = zext i64 {lo} to i128"));
                self.line(format!("{wh} = zext i64 {hi} to i128"));
                self.line(format!("{sh} = shl i128 {wh}, 64"));
                self.line(format!("{dst} = or i128 {sh}, {wl}"));
            }
            Op::Stxp { addr, bytes, lo, hi } => {
                self.m.runtime.insert("recomp_stxp");
                let (a, l, h) = (self.val(addr), self.val(lo), self.val(hi));
                self.line(format!("{dst} = call i32 @recomp_stxp(ptr %c, i64 {a}, i32 {bytes}, i64 {l}, i64 {h})"));
            }
            Op::Clrex => {
                self.m.runtime.insert("recomp_clrex");
                self.line("call void @recomp_clrex(ptr %c)");
            }
            Op::Counter => {
                self.m.runtime.insert("recomp_cntpct");
                self.line(format!("{dst} = call i64 @recomp_cntpct(ptr %c)"));
            }
            Op::Call { target } => {
                let (spill, reload) = self.around(v);
                self.spill(&spill);
                self.call_guest(target);
                self.reload(&reload);
            }
            Op::SetjmpCall { target, rets } => {
                // (host/hos_setjmp.c: the slot pushed, a host setjmp on it; a guest longjmp to one of the owner's
                // setjmp return addresses comes back with 1, and the owner's code runs from there)
                let (spill, reload) = self.around(v);
                self.spill(&spill);
                for name in ["recomp_sj_push", "recomp_sj_setjmp", "recomp_sj_target", "recomp_sj_pop", "a64_icall"] {
                    self.m.runtime.insert(name);
                }
                let (slot, jumped, first) = (self.temp(), self.temp(), self.temp());
                let (call, resume, join) = (self.fresh_label(), self.fresh_label(), self.fresh_label());
                self.line(format!(
                    "{slot} = call ptr @recomp_sj_push(ptr %c, i64 {}, i64 {}, i64 {}, i64 {})",
                    rets[0], rets[1], rets[2], rets[3]
                ));
                self.line(format!("{jumped} = call i32 @recomp_sj_setjmp(ptr {slot}) returns_twice"));
                self.line(format!("{first} = icmp eq i32 {jumped}, 0"));
                self.line(format!("br i1 {first}, label %{call}, label %{resume}"));
                self.label(&call);
                self.call_guest(target);
                self.line(format!("br label %{join}"));
                self.label(&resume);
                let at = self.temp();
                self.line(format!("{at} = call i64 @recomp_sj_target(ptr {slot})"));
                self.line(format!("call void @a64_icall(ptr %c, i64 {at})"));
                self.line(format!("br label %{join}"));
                self.label(&join);
                self.line(format!("call void @recomp_sj_pop(ptr {slot})"));
                self.reload(&reload);
            }
            Op::CallIndirect { target, usual } => {
                let (spill, reload) = self.around(v);
                self.spill(&spill);
                let t = self.val(target);
                match usual {
                    Some(u) => {
                        let hit = self.temp();
                        let (direct, indirect, join) = (self.fresh_label(), self.fresh_label(), self.fresh_label());
                        let (direct, indirect, join) = (direct.as_str(), indirect.as_str(), join.as_str());
                        self.line(format!("{hit} = icmp eq i64 {t}, {}", u.address));
                        self.line(format!("br i1 {hit}, label %{direct}, label %{indirect}, !prof !0"));
                        self.label(direct);
                        self.call_guest(u.callee);
                        self.line(format!("br label %{join}"));
                        self.label(indirect);
                        self.site_call(&t, "a64_icall_site");
                        self.line(format!("br label %{join}"));
                        self.label(join);
                    }
                    None => self.site_call(&t, "a64_icall_site"),
                }
                self.reload(&reload);
            }
            Op::Param { .. } => {} // (named by the helper's definition)
            Op::FClass(class, a) => {
                let ty = self.ty(a);
                let mask = match class {
                    FClass::Subnormal => FPCLASS_SUBNORMAL,
                    FClass::NanOrSubnormal => FPCLASS_SUBNORMAL | FPCLASS_NAN,
                };
                let args = [self.arg(a), ("i32".to_string(), mask.to_string())];
                self.call_intrinsic(&dst, "is.fpclass", ty.with_elem(Type::I1), &[ty], &args);
            }
            Op::Checked { fast, slow, helper, args } => {
                let f: &Function = self.f;
                let h = f.helper(helper);
                let ty = llvm_type(self.ty(v));
                let cond = self.val(slow);
                let (call, join) = (self.temp(), self.temp());
                let (call, join) = (format!("h{}", &call[2..]), format!("j{}", &join[2..]));
                self.count_check("@lifter_fast_checks");
                let from = self.at.clone();
                let unlikely = self.m.unlikely();
                self.line(format!("br i1 {cond}, label %{call}, label %{join}, !prof {unlikely}"));
                self.label(&call);
                self.count_check("@lifter_slow_checks");
                let r = self.temp();
                let args: Vec<String> = args.iter().map(|&a| self.typed(a)).collect();
                self.line(format!("{r} = call {ty} @{}({})", h.name, args.join(", ")));
                self.line(format!("br label %{join}"));
                self.label(&join);
                let fast = self.val(fast);
                self.line(format!("{dst} = phi {ty} [ {fast}, %{from} ], [ {r}, %{call} ]"));
            }
            Op::Svc { imm } => {
                let (spill, reload) = self.around(v);
                self.spill(&spill);
                self.m.runtime.insert("a64_svc");
                self.line(format!("call void @a64_svc(ptr %c, i32 {imm})"));
                self.reload(&reload);
            }
            Op::HostLoad { global, ty } => {
                let t = llvm_type(ty);
                self.m.host_global(&global, &t);
                let p = self.global_ptr(&global);
                self.line(format!("{dst} = load volatile {t}, ptr {p}, align {}", ty.bits() / 8));
            }
            Op::HostStore { global, value } => {
                let ty = self.ty(value);
                self.m.host_global(&global, &llvm_type(ty));
                let p = self.global_ptr(&global);
                let x = self.typed(value);
                self.line(format!("store volatile {x}, ptr {p}, align {}", ty.bits() / 8));
            }
            Op::HostCall { callee, args, ret } => {
                let with_state = args.contains(&HostArg::Ctx);
                let around = with_state.then(|| self.around(v));
                if let Some((spill, _)) = &around {
                    self.spill(spill);
                }
                self.host_call(&dst, &callee, &args, ret);
                if let Some((_, reload)) = &around {
                    self.reload(reload);
                }
            }
        }
    }

    /// A pointer to the host global `g`: the global itself, or this thread's copy of a thread-local one, asked anew
    /// at each access from an accessor LLVM cannot see into (`@tls.<symbol>`, weak: any unit's copy may be the one
    /// linked). `llvm.threadlocal.address` is a pure function of its global to LLVM, which may reuse one address across
    /// a call; a guest thread's fiber can resume on another host thread across a call, with another copy.
    fn global_ptr(&mut self, g: &Global) -> String {
        if !g.thread_local {
            return format!("@{}", g.symbol);
        }
        self.m.intrinsics.insert("declare ptr @llvm.threadlocal.address.p0(ptr)".into());
        let sym = &g.symbol;
        self.m.intrinsics.insert(format!(
            "define weak hidden ptr @tls.{sym}() noinline nounwind {{\n  %p = call ptr @llvm.threadlocal.address.p0(ptr @{sym})\n  ret ptr %p\n}}"
        ));
        let p = self.temp();
        self.line(format!("{p} = call ptr @tls.{sym}()"));
        p
    }

    /// A host function's call (the C ABI): `dst` its value when it returns one.
    fn host_call(&mut self, dst: &str, callee: &Callee, args: &[HostArg], ret: Option<Type>) {
        let mut params = Vec::new();
        let mut operands = Vec::new();
        for a in args {
            match a {
                HostArg::Ctx => {
                    params.push("ptr".to_string());
                    operands.push("ptr %c".to_string());
                }
                HostArg::Value(x) => {
                    params.push(llvm_type(self.ty(*x)));
                    operands.push(self.typed(*x));
                }
                HostArg::Function(name) => {
                    self.m.host_function(name, format!("declare void @{name}(ptr)"));
                    params.push("ptr".to_string());
                    operands.push(format!("ptr @{name}"));
                }
            }
        }
        let r = ret.map_or_else(|| "void".to_string(), llvm_type);
        let target = match callee {
            Callee::Named(name) => {
                self.m.host_function(name, format!("declare {r} @{name}({})", params.join(", ")));
                format!("@{name}")
            }
            Callee::Slot(g) => {
                self.m.host_global(g, "ptr");
                let p = self.global_ptr(g);
                let fp = self.temp();
                self.line(format!("{fp} = load volatile ptr, ptr {p}, align 8"));
                fp
            }
        };
        let call = format!("call {r} {target}({})", operands.join(", "));
        if ret.is_some() { self.line(format!("{dst} = {call}")) } else { self.line(call) }
    }

    /// With [`Options::count_checks`], one more in the counter `global`.
    fn count_check(&mut self, global: &str) {
        if self.m.opts.count_checks {
            let (old, new) = (self.temp(), self.temp());
            self.line(format!("{old} = load i64, ptr {global}, align 8"));
            self.line(format!("{new} = add i64 {old}, 1"));
            self.line(format!("store i64 {new}, ptr {global}, align 8"));
        }
    }

    /// A call through a site's two-slot cache (rt/a64.h's A64_SITE_CALL): the target's function straight from the
    /// cache when its first slot holds it, else `slow` (a64_icall_site or a64_ijump_site), which fills it.
    fn site_call(&mut self, target: &str, slow: &'static str) {
        self.sites += 1;
        let site = format!("@s.{}.{}", self.f.name(), self.sites);
        let _ = writeln!(self.m.globals, "{site} = internal global [2 x i64] zeroinitializer, align 8");
        self.m.runtime.insert(slow);
        self.m.runtime.insert("a64_lookup");
        let (e, top, same, set, hit) = (self.temp(), self.temp(), self.temp(), self.temp(), self.temp());
        let (fast, miss, join) = (self.fresh_label(), self.fresh_label(), self.fresh_label());
        self.line(format!("{e} = load atomic i64, ptr {site} monotonic, align 8"));
        self.line(format!("{top} = lshr i64 {e}, 32"));
        self.line(format!("{same} = icmp eq i64 {top}, {target}"));
        self.line(format!("{set} = icmp ne i64 {e}, 0"));
        self.line(format!("{hit} = and i1 {same}, {set}"));
        self.line(format!("br i1 {hit}, label %{fast}, label %{miss}, !prof !0"));
        self.label(&fast);
        let (lo, off, base, at, fnp) = (self.temp(), self.temp(), self.temp(), self.temp(), self.temp());
        self.line(format!("{lo} = trunc i64 {e} to i32"));
        self.line(format!("{off} = sext i32 {lo} to i64"));
        self.line(format!("{base} = ptrtoint ptr @a64_lookup to i64"));
        self.line(format!("{at} = add i64 {base}, {off}"));
        self.line(format!("{fnp} = inttoptr i64 {at} to ptr"));
        self.line(format!("call void {fnp}(ptr %c)"));
        self.line(format!("br label %{join}"));
        self.label(&miss);
        self.line(format!("call void @{slow}(ptr %c, i64 {target}, ptr {site})"));
        self.line(format!("br label %{join}"));
        self.label(&join);
    }

    /// The soft write watch's marks for `bytes` bytes at `addr`: its first and its last byte's pages (A64_MARK).
    fn mark(&mut self, addr: Value, bytes: u32) {
        if bytes == 0 {
            return; // (no byte written: no page)
        }
        let a = self.val(addr);
        let pages = self.m.opts.dirty_pages - 1;
        let (first, first_page) = (self.temp(), self.temp());
        self.line(format!("{first} = lshr i64 {a}, 12"));
        self.line(format!("{first_page} = and i64 {first}, {pages}"));
        let (end, last, last_page) = (self.temp(), self.temp(), self.temp());
        self.line(format!("{end} = add i64 {a}, {}", bytes - 1));
        self.line(format!("{last} = lshr i64 {end}, 12"));
        self.line(format!("{last_page} = and i64 {last}, {pages}"));
        let done = if self.m.opts.mark_bitmap {
            // (only a store that writes a page the GPU maps marks: host/mark_bitmap.c's bit per page, of its first
            // page or its last (a shared mark may start on a CPU page and end on a GPU one))
            let mut on = Vec::new();
            for page in [&first_page, &last_page] {
                let (byte_at, byte_p, byte, shift, shift8, bits, bit) =
                    (self.temp(), self.temp(), self.temp(), self.temp(), self.temp(), self.temp(), self.temp());
                self.line(format!("{byte_at} = lshr i64 {page}, 3"));
                self.line(format!("{byte_p} = getelementptr inbounds i8, ptr @a64_gpu_pages, i64 {byte_at}"));
                self.line(format!("{byte} = load i8, ptr {byte_p}, align 1"));
                self.line(format!("{shift} = and i64 {page}, 7"));
                self.line(format!("{shift8} = trunc i64 {shift} to i8"));
                self.line(format!("{bits} = lshr i8 {byte}, {shift8}"));
                self.line(format!("{bit} = trunc i8 {bits} to i1"));
                on.push(bit);
            }
            let either = self.temp();
            self.line(format!("{either} = or i1 {}, {}", on[0], on[1]));
            let (yes, done) = (self.fresh_label(), self.fresh_label());
            self.line(format!("br i1 {either}, label %{yes}, label %{done}"));
            self.label(&yes);
            Some(done)
        } else {
            None
        };
        for page in [first_page, last_page] {
            let p = self.temp();
            self.line(format!("{p} = getelementptr inbounds i8, ptr @a64_dirty, i64 {page}"));
            self.line(format!("store atomic i8 1, ptr {p} release, align 1"));
        }
        if let Some(done) = done {
            self.line(format!("br label %{done}"));
            self.label(&done);
        }
    }

    /// The label an edge's branch goes to: its block's, or a trampoline's that does the edge's work first.
    fn edge_label(&mut self, e: Edge) -> String {
        match e.to {
            Dest::Block(b) if !(e.poll && self.m.opts.polls) => format!("%b{}", b.0),
            _ => {
                let t = self.temp();
                let label = format!("e{}", &t[2..]);
                self.trampolines.push((label.clone(), e));
                format!("%{label}")
            }
        }
    }

    /// An edge's own code, ending in its jump (a poll, a tail call, a trap).
    fn edge_code(&mut self, e: Edge) {
        match e.to {
            Dest::Block(b) => {
                if e.poll && self.m.opts.polls {
                    self.poll();
                }
                self.line(format!("br label %b{}", b.0));
            }
            Dest::Tail(t) => {
                self.spill_exit();
                self.m.called.insert(t);
                self.line(format!("musttail call void @f_{t:08x}(ptr %c)"));
                self.line("ret void");
            }
            Dest::BadTarget(t) => self.trap(t, 0),
            Dest::Piece { piece, entry } => {
                let base = self.f.pieces.as_ref().expect("a piece's edge, in a piece (verified)").base.clone();
                // (a loop's back edge that a cut made a piece edge still polls)
                if e.poll && self.m.opts.polls {
                    self.poll();
                }
                self.spill_exit();
                let p = self.ctx_ptr(ctx::PIECE);
                self.line(format!("store i32 {entry}, ptr {p}, align 4"));
                self.line(format!("musttail call void @{}(ptr %c)", mir::Pieces::symbol(&base, piece)));
                self.line("ret void");
            }
        }
    }

    /// A64_POLL: when the context's interrupt flag is set, clear it and hand the core on.
    fn poll(&mut self) {
        self.m.runtime.insert("a64_poll_cur");
        let p = self.ctx_ptr(ctx::POLL);
        let (flag, set) = (self.temp(), self.temp());
        let (yes, done) = (self.fresh_label(), self.fresh_label());
        self.line(format!("{flag} = load atomic i32, ptr {p} monotonic, align 4"));
        self.line(format!("{set} = icmp ne i32 {flag}, 0"));
        self.line(format!("br i1 {set}, label %{yes}, label %{done}"));
        self.label(&yes);
        self.line(format!("store atomic i32 0, ptr {p} monotonic, align 4"));
        self.line("call void @a64_poll_cur()");
        self.line(format!("br label %{done}"));
        self.label(&done);
    }

    fn trap(&mut self, pc: u64, word: u32) {
        self.spill_exit();
        self.m.runtime.insert("a64_trap");
        self.line(format!("call void @a64_trap(ptr %c, i64 {pc}, i32 {word})"));
        self.line("ret void");
    }

    fn term(&mut self, term: &Term) {
        match term {
            Term::Jump(e) => {
                let l = self.edge_label(*e);
                self.line(format!("br label {l}"));
            }
            Term::Branch { cond, then, otherwise } => {
                let c = self.val(*cond);
                let block = self.f.block(BlockId(self.cur as u32));
                let (pc, guest) = (block.pc, block.guest_term);
                if guest {
                    self.count(profile::Counter::Taken(pc), Some(&c));
                }
                let (t, f) = (self.edge_label(*then), self.edge_label(*otherwise));
                let weights = if guest { self.weights(pc) } else { String::new() };
                self.line(format!("br i1 {c}, label {t}, label {f}{weights}"));
            }
            Term::Switch { value, cases } => {
                let v = self.val(*value);
                let default = self.fresh_label();
                let mut arms = String::new();
                for (addr, e) in cases {
                    let l = self.edge_label(*e);
                    let _ = write!(arms, " i64 {addr}, label {l}");
                }
                self.line(format!("switch i64 {v}, label %{default} [{arms} ]"));
                self.label(&default);
                self.indirect_jump(&v);
            }
            Term::Return => {
                self.spill_exit();
                self.line("ret void");
            }
            Term::IndirectJump(v) => {
                let v = self.val(*v);
                self.indirect_jump(&v);
            }
            Term::Trap { pc, word } => self.trap(*pc, *word),
        }
    }

    /// A BR that is no table (A64_IJUMP): the target's function called through the site's cache, then return.
    fn indirect_jump(&mut self, target: &str) {
        self.spill_exit();
        self.site_call(target, "a64_ijump_site");
        self.line("ret void");
    }
}

#[cfg(test)]
mod tests;

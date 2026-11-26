//! The lift's other files in gen/: the function table, the boot constants, the import stubs and the runtime. Each is
//! made from the image and the discovery alone (docs/dev/reference/builder.md, "The gen/ folder").

use super::host::{Host, nvn_proc_name};
use cfg::plan::Plan;
use image::Image;
use std::collections::BTreeSet;
use std::fmt::Write as _;

/// The runtime's files, as gen/rt/ holds them (builder/rt/).
pub const RUNTIME: &[(&str, &str)] = &[
    ("a64.h", include_str!("../../../../rt/a64.h")),
    ("a64_rt.c", include_str!("../../../../rt/a64_rt.c")),
    ("a64_fp.c", include_str!("../../../../rt/a64_fp.c")),
];

/// `rt/a64_config.h`: the lift's choices the host must be compiled with, as the IR was written with them.
pub fn config_h(o: &llvm::Options) -> String {
    format!(
        "/* Written by longshot-builder: the choices this lift's IR was written with (builder/crates/llvm Options). The host\n   \
         is compiled against them through a64.h: do not define these on the command line. */\n\
         #ifndef A64_CONFIG_H\n#define A64_CONFIG_H\n\
         #define A64_HOST_BASE {:#x}ULL   /* where guest address 0 is */\n\
         #define A64_DIRTY_PAGES {:#x}ULL   /* the page marks' table: a byte per 4 KB page */\n\
         #define A64_WRITE_TRACK 1   /* (stores mark their pages: always) */\n\
         #define A64_MARK_BITMAP {}   /* only stores whose first or last byte is on a GPU page mark */\n\
         #define A64_POLLING {}   /* back edges poll for an interrupt */\n\
         #endif\n",
        o.memory_base,
        o.dirty_pages,
        u8::from(o.mark_bitmap),
        u8::from(o.polls),
    )
}

/// `table.c`: every function's entry and code, ascending (a64_rt.c's `a64_lookup` searches it), and each hooked
/// address's `f_<address>`, a tail call of its host function.
pub fn table_c(plan: &Plan) -> String {
    let fns: BTreeSet<u64> = plan.functions.keys().chain(plan.hooks.keys()).copied().collect();
    let mut s = String::with_capacity(fns.len() * 64);
    s.push_str("/* Written by longshot-builder: the function table and the hooked addresses. */\n#include \"a64.h\"\n");
    for a in &fns {
        let _ = writeln!(s, "void f_{a:08x}(Ctx *A64_NOESC);");
    }
    let _ = writeln!(s, "const uint32_t a64_fn_count = {};", fns.len());
    s.push_str("const uint32_t a64_fn_addr[] = {\n");
    for a in &fns {
        let _ = writeln!(s, "0x{a:08x},");
    }
    s.push_str("};\nconst A64Fn a64_fn_ptr[] = {\n");
    for a in &fns {
        let _ = writeln!(s, "f_{a:08x},");
    }
    s.push_str("};\n");
    let names: BTreeSet<&String> = plan.hooks.values().collect();
    for n in names {
        let _ = writeln!(s, "void {n}(Ctx *A64_NOESC);");
    }
    for (a, n) in &plan.hooks {
        let _ = writeln!(s, "void f_{a:08x}(Ctx *A64_NOESC c) {{ TAILCALL({n}); }}");
    }
    s
}

/// `imports.c`: a stub for each hooked symbol the host does not define (it logs its first call and returns 0), and
/// each NVN function routed to the host's driver (host/nvn_native.c `nvn2_dispatch`). Also the counts of each.
pub fn imports_c(image: &Image, host: &Host) -> (String, usize, usize) {
    let mut s = String::from(
        "/* Written by longshot-builder: the imports the host does not define, and the NVN functions it routes. */\n\
         #include \"a64.h\"\n\
         void a64_unimplemented(Ctx *A64_NOESC c, const char *sym, int *seen);\n\
         void nvn2_dispatch(Ctx *c, const char *name, void **cache);\n",
    );
    let (mut stubs, mut nvn) = (BTreeSet::new(), BTreeSet::new());
    for h in image.hooks() {
        if host.implemented.contains(&h.name) {
            continue;
        }
        match nvn_proc_name(&h.sym) {
            Some(proc) => {
                if nvn.insert(h.name.clone()) {
                    let _ = writeln!(
                        s,
                        "void {}(Ctx *A64_NOESC c) {{ static void *k[2]; nvn2_dispatch(c, \"{proc}\", k); }}",
                        h.name
                    );
                }
            }
            None => {
                if stubs.insert(h.name.clone()) {
                    let _ = writeln!(
                        s,
                        "void {}(Ctx *A64_NOESC c) {{ static int seen; a64_unimplemented(c, \"{}\", &seen); }}",
                        h.name, h.sym
                    );
                }
            }
        }
    }
    (s, stubs.len(), nvn.len())
}

/// The SDK thread entry's start and exit hooks (the C library's per-thread TLS setup and teardown): the entry starts
/// `mrs x1, tpidrro_el0; add x1, x1, #0x1f8; str x0, [x1]` and loads each hook through a GOT slot. `[0, 0]` when no
/// such entry is found.
pub fn thread_hooks(image: &Image) -> [u64; 2] {
    let word = |pc: u64| image.word(pc).unwrap_or(0);
    for text in image.code() {
        let mut pc = text.start;
        while pc + 12 <= text.end {
            if word(pc) == 0xD53B_D061 && word(pc + 4) == 0x9107_E021 && word(pc + 8) == 0xF900_0020 {
                let (mut hooks, mut page) = (Vec::new(), [None::<u64>; 32]);
                for q in (pc + 12..pc + 12 + 4 * 40).step_by(4) {
                    let w = word(q);
                    if w & 0x9F00_0000 == 0x9000_0000 {
                        let imm = (((w >> 3) & 0x1F_FFFC) | ((w >> 29) & 3)) as i64;
                        let imm = if imm & (1 << 20) != 0 { imm - (1 << 21) } else { imm };
                        page[(w & 31) as usize] = Some((q & !0xFFF).wrapping_add((imm << 12) as u64));
                    } else if w & 0xFFC0_0000 == 0xF940_0000
                        && let Some(base) = page[((w >> 5) & 31) as usize].take()
                    {
                        let got = base + u64::from((w >> 10) & 0xFFF) * 8;
                        hooks.push(image.u64(got).unwrap_or(0));
                    }
                    if hooks.len() == 2 {
                        return [hooks[0], hooks[1]];
                    }
                }
            }
            pc += 4;
        }
    }
    [0, 0]
}

/// What `main.npdm` says of the main thread: its stack, priority and core, and the program id (its ACI0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Npdm {
    /// The main thread's stack size.
    pub stack: u64,
    /// Its priority.
    pub priority: u8,
    /// Its core.
    pub core: u8,
    /// The program id (0 without an ACI0).
    pub program_id: u64,
}

impl Npdm {
    /// The fields of a `main.npdm`, or why it is not one.
    pub fn parse(b: &[u8]) -> Result<Npdm, String> {
        let u32_at = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().expect("4 bytes")));
        if b.get(..4) != Some(b"META") || b.len() < 0x80 {
            return Err("no META header".into());
        }
        let (aoff, asz) = (u32_at(0x70).unwrap_or(0) as usize, u32_at(0x74).unwrap_or(0) as usize);
        let aci = b.get(aoff..aoff + asz).unwrap_or(&[]);
        let program_id = match (aci.get(..4), aci.get(0x10..0x18)) {
            (Some(b"ACI0"), Some(id)) => u64::from_le_bytes(id.try_into().expect("8 bytes")),
            _ => 0,
        };
        Ok(Npdm { stack: u64::from(u32_at(0x1C).unwrap_or(0)), priority: b[0xE], core: b[0xF], program_id })
    }
}

/// `image.c`: the boot constants host/hos_main.c reads: the image's span, each module's DT_INIT in load order (rtld
/// left out: the image is relocated already), the entry points, the main thread's stack, priority and core, the main
/// module's build id and extent, and the thread hooks.
pub fn image_c(image: &Image, npdm: &Npdm) -> String {
    let ex = |name: &str| image.exports().get(name).copied().unwrap_or(0);
    let mods: Vec<(&image::Module, u64)> = image
        .modules()
        .iter()
        .zip(image.inits())
        .filter(|(m, init)| **init != 0 && m.name != "rtld")
        .map(|(m, &init)| (m, init))
        .collect();
    let main = image.module("main");
    let mut s = String::from("/* Written by longshot-builder: the image's boot constants. */\n#include <stdint.h>\n");
    let _ = writeln!(s, "const uint64_t hos_main_stack_size = {:#x}ULL;", npdm.stack);
    let _ = writeln!(s, "const int hos_main_priority = {}, hos_main_core = {};", npdm.priority, npdm.core);
    let _ =
        writeln!(s, "const uint64_t hos_image_base = {:#x}ULL, hos_image_end = {:#x}ULL;", image.base(), image.end());
    let _ = writeln!(s, "const int hos_module_count = {};", mods.len());
    let inits: Vec<String> = mods.iter().map(|(_, i)| format!("{i:#x}ULL")).collect();
    let _ = writeln!(s, "const uint64_t hos_module_init[] = {{{}}};", inits.join(", "));
    let names: Vec<String> = mods.iter().map(|(m, _)| format!("\"{}\"", m.name)).collect();
    let _ = writeln!(s, "const char *const hos_module_name[] = {{{}}};", names.join(", "));
    let _ = writeln!(s, "const uint64_t hos_entry_nninitStartup = {:#x}ULL;", ex("nninitStartup"));
    let _ = writeln!(s, "const uint64_t hos_entry_nnMain = {:#x}ULL;", ex("nnMain"));
    let _ = writeln!(s, "const uint64_t hos_program_id = {:#x}ULL;", npdm.program_id);
    let _ = writeln!(s, "const char hos_main_build_id[] = \"{}\";", main.map(|m| m.build_id.as_str()).unwrap_or(""));
    match main {
        Some(m) => {
            let ro = m.segments.iter().find(|s| s.name == ".rodata").map(|s| s.addr).unwrap_or(m.base + m.size);
            let _ = writeln!(
                s,
                "const uint64_t hos_main_base = {:#x}ULL, hos_main_size = {:#x}ULL, hos_main_code_end = {ro:#x}ULL;",
                m.base, m.size
            );
        }
        None => s.push_str("const uint64_t hos_main_base = 0, hos_main_size = 0, hos_main_code_end = 0;\n"),
    }
    for sym in [
        "_init_libc0",
        "_init_libc1",
        "_init_libc2",
        "nnosInitialize",
        "nninitInitializeSdkModule",
        "nvnBootstrapLoaderInternal",
    ] {
        let _ = writeln!(s, "const uint64_t hos_entry_{} = {:#x}ULL;", sym.trim_start_matches('_'), ex(sym));
    }
    let [start, exit] = thread_hooks(image);
    let _ = writeln!(s, "const uint64_t hos_thread_start_hook = {start:#x}ULL, hos_thread_exit_hook = {exit:#x}ULL;");
    s
}

/// `functions.txt`: a line per function, `<entry> <instructions> <unit>` (hex, decimal, the unit's file or `held`).
pub fn functions_txt(plan: &Plan, unit_of: &dyn Fn(u64) -> Option<String>) -> String {
    let mut s = String::with_capacity(plan.functions.len() * 32);
    for (e, f) in &plan.functions {
        let unit = unit_of(*e).map(|u| format!("{u}.ll")).unwrap_or_else(|| "held".into());
        let _ = writeln!(s, "{e:08x} {} {unit}", f.len());
    }
    s
}

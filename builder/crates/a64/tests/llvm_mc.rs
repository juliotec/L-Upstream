//! The decoder against LLVM's disassembler, word for word: random words in every encoding group (and words with each
//! group's selector bits fixed), each decoded here and by `llvm-mc --disassemble -mcpu=cortex-a78c -M no-aliases`.
//! Both must agree on whether the word is an instruction, and on its text.
//!
//! Any llvm-mc from LLVM 18 on: CI installs Ubuntu 24.04's (LLVM 18), which names PRFM's SLC operations (`pldslckeep`)
//! only with `-mattr=+prfm-slc-target` (newer LLVM names them either way), and reads two more encodings as an older
//! architecture did (`older_llvm`).
//!
//! What is not compared: the system registers this decoder does not name (llvm-mc names thousands), and the SYS
//! aliases (DC, IC, AT, TLBI).
//!
//! Needs llvm-mc: the one `LLVM_MC` names, which must run (tests/test_lifter.py sets it), else the one on PATH; with
//! neither, the check says so and passes. `LIFTER_WORDS=<n>` sets the words per group (default 40000).

use a64::{Insn, decode};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }
}

fn llvm_mc() -> Option<String> {
    if let Ok(p) = std::env::var("LLVM_MC") {
        return Some(p);
    }
    let found = Command::new("llvm-mc").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status();
    found.ok().filter(|s| s.success()).map(|_| "llvm-mc".to_string())
}

/// llvm-mc's text for each word, `None` where it says the encoding is invalid.
fn disassemble(mc: &str, words: &[u32]) -> Vec<Option<String>> {
    let mut input = String::with_capacity(words.len() * 20);
    for w in words {
        let b = w.to_le_bytes();
        input.push_str(&format!("0x{:02x} 0x{:02x} 0x{:02x} 0x{:02x}\n", b[0], b[1], b[2], b[3]));
    }
    let mut child = Command::new(mc)
        .args(["--disassemble", "-triple=aarch64", "-mcpu=cortex-a78c", "-mattr=+prfm-slc-target", "-M", "no-aliases"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("llvm-mc starts");
    let mut stdin = child.stdin.take().unwrap();
    let feeder = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output().expect("llvm-mc runs");
    feeder.join().unwrap().unwrap();
    let mut invalid = HashSet::new();
    for line in String::from_utf8_lossy(&out.stderr).lines() {
        // <stdin>:LINE:COL: warning: invalid instruction encoding
        if line.contains("invalid instruction encoding") {
            let n: usize = line.split(':').nth(1).and_then(|s| s.parse().ok()).expect("a line number");
            invalid.insert(n - 1);
        }
    }
    let mut texts = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('.'))
        .map(normalize)
        .collect::<Vec<_>>()
        .into_iter();
    (0..words.len())
        .map(|i| if invalid.contains(&i) { None } else { Some(texts.next().expect("a line per word")) })
        .collect()
}

/// llvm-mc decodes some later architecture's encodings whatever `-mcpu` says; on the A78C they are unallocated (or,
/// for RPRFM, a PRFM of a reserved operation, which is how this decoder reads them).
fn later_architecture(insn: Option<&Insn>, theirs: &str) -> bool {
    let mnemonic = theirs.split(' ').next().unwrap_or("");
    match insn {
        None => {
            matches!(mnemonic, "sysp" | "mrrs" | "msrr")
                // (MRS and MSR with op0 0: SYSREG128 space; SME's SVCR fields)
                || (matches!(mnemonic, "mrs" | "msr") && (theirs.contains("S0_") || theirs.contains("SVCR")))
        }
        Some(Insn::LoadStore { kind: a64::LsKind::Prefetch, .. }) => mnemonic == "rprfm",
        _ => false,
    }
}

/// Where LLVM 18's llvm-mc reads a word as an older architecture did and newer LLVM agrees with this decoder: it rejects
/// LDAPR, LDAPRB and LDAPRH whose should-be-one Rs bits (20:16) are not all ones (newer LLVM decodes them, warning
/// "potentially undefined"), and it has no name for PRFM's IR operation (#24).
fn older_llvm(word: u32, ours: &str, theirs: Option<&str>) -> bool {
    let mnemonic = ours.split(' ').next().unwrap_or("");
    match theirs {
        None => matches!(mnemonic, "ldapr" | "ldaprb" | "ldaprh") && (word >> 16) & 0x1f != 0x1f,
        Some(t) => {
            matches!(mnemonic, "prfm" | "prfum") && ours.contains(" ir,") && t == ours.replacen(" ir,", " #24,", 1)
        }
    }
}

fn normalize(s: &str) -> String {
    let s = s.split("//").next().unwrap();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether the two texts differ only where the check does not compare.
fn tolerated(insn: &Insn, ours: &str, theirs: &str) -> bool {
    match insn {
        Insn::Sys { l: false, .. } => !theirs.starts_with("sys "),
        &Insn::Mrs { reg, .. } | &Insn::Msr { reg, .. } => {
            // our generic name (S3_3_C4_C2_0) where llvm-mc has a name: every other token the same
            let generic = a64::print::generic_sysreg(reg);
            let tokens = |t: &str| t.split([' ', ',']).filter(|p| !p.is_empty()).map(String::from).collect::<Vec<_>>();
            let (a, b) = (tokens(ours), tokens(theirs));
            ours.contains(&generic) && a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x == y || *x == generic)
        }
        _ => false,
    }
}

/// The words to check: for each top-level group (bits 28:25), random words, and random words with its sub-groups'
/// selector bits fixed (so that small groups like PAuth or the exception generators are hit often).
fn words(per_group: usize) -> Vec<u32> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::new();
    let groups: &[(u32, u32)] = &[
        // (mask, value)
        (0x1e00_0000, 0x1000_0000),           // data processing, immediate (100x)
        (0x1c00_0000, 0x1400_0000),           // branches, system (101x)
        (0x0a00_0000, 0x0800_0000),           // loads and stores (x1x0)
        (0x0e00_0000, 0x0a00_0000),           // data processing, register (x101)
        (0xffe0_0000, 0xd400_0000),           // exception generation
        (0xffc0_0000, 0xd500_0000),           // system (hints, barriers, PSTATE, SYS, MRS, MSR)
        (0xfe00_0000, 0xd600_0000),           // branches to a register (incl. PAuth)
        (0x3f00_0000, 0x0800_0000),           // exclusive, ordered, compare and swap
        (0x3f20_0c00, 0x3820_0000),           // atomics
        (0x3f20_0400, 0x3820_0400),           // LDRAA, LDRAB
        (0x3f20_0c00, 0x3820_0800),           // register offset
        (0x3b00_0000, 0x1800_0000),           // literal
        (0x7fe0_0000, 0x5ac0_0000),           // data processing, 1 source (incl. PAuth)
        (0x5fe0_0000, 0x1ac0_0000),           // data processing, 2 sources
        (0x1fe0_0000, 0x1a00_0000),           // carry, RMIF, SETF
        (0x1fe0_0000, 0x1a40_0000),           // conditional compare
        (0x1fe0_0000, 0x1a80_0000),           // conditional select
        (0x1f00_0000, 0x1b00_0000),           // 3 sources
        (0x1e00_0000, 0x0e00_0000),           // floating point and Advanced SIMD (x111)
        (0x5f20_0000, 0x1e20_0000),           // scalar FP (incl. conversions)
        (0x5f00_0000, 0x1f00_0000),           // FP 3 sources
        (0x5f20_0000, 0x1e00_0000),           // FP and fixed-point conversions
        (0x9f20_0400, 0x0e20_0400),           // vector three same
        (0x9f20_0c00, 0x0e20_0000),           // vector three different
        (0x9f3e_0c00, 0x0e20_0800),           // vector two-register miscellaneous
        (0x9f3e_0c00, 0x0e30_0800),           // across lanes
        (0x9f60_c400, 0x0e40_0400),           // three same FP16
        (0x9f7e_0c00, 0x0e78_0800),           // two-register miscellaneous FP16
        (0x9f20_8400, 0x0e00_8400),           // three same extra
        (0x9fe0_8400, 0x0e00_0400),           // copy
        (0xbf20_8000, 0x0e00_0000),           // table, permute, extract
        (0x9ff8_0400, 0x0f00_0400),           // modified immediate
        (0x9f80_0400, 0x0f00_0400),           // shift by immediate
        (0x9f00_0400, 0x0f00_0000),           // by element
        (0xdf00_0000, 0x5e00_0000),           // scalar Advanced SIMD, crypto
        (0xdf00_0400, 0x5f00_0000),           // scalar shift, by element
        (0xbf80_0000, 0x0c00_0000),           // structure loads and stores, multiple
        (0xbf80_0000, 0x0d00_0000),           // structure loads and stores, single
        (0xbf80_0000, 0x0c80_0000),           // structure loads and stores, multiple, post-index
        (0xbf80_0000, 0x0d80_0000),           // structure loads and stores, single, post-index
        (0x3b00_0000 | 1 << 26, 0x3d00_0000), // loads and stores of SIMD and FP registers, unsigned offset
        (0x3a00_0000 | 1 << 26, 0x2c00_0000), // pairs of SIMD and FP registers
        (0x0000_0000, 0x0000_0000),           // anything
    ];
    for &(mask, value) in groups {
        for _ in 0..per_group {
            out.push((rng.next() & !mask) | value);
        }
    }
    // Words with few bits set reach the fixed fields (SBO and SBZ) that random words almost never match.
    for _ in 0..per_group {
        let (base, mask) = groups[(rng.next() as usize) % groups.len()];
        let sparse = rng.next() & rng.next() & rng.next();
        let dense = rng.next() | rng.next() | rng.next();
        out.push(((sparse & !base) | mask) & !(base & !mask));
        out.push(((dense & !base) | mask) & !(base & !mask));
    }
    out
}

#[test]
fn matches_llvm_mc() {
    let Some(mc) = llvm_mc() else {
        // (a PC or CI job without llvm-mc fails here rather than pass unchecked: the rule lift's, llvm's and
        // site_hooks' tests follow for clang)
        assert_eq!(
            std::env::var("LONGSHOT_NO_CLANG").as_deref(),
            Ok("1"),
            "no llvm-mc to compare the decoder with (set LLVM_MC; LONGSHOT_NO_CLANG=1 passes over this check)"
        );
        eprintln!("llvm_mc: skipped, llvm-mc not found and LONGSHOT_NO_CLANG=1");
        return;
    };
    let per_group = std::env::var("LIFTER_WORDS").ok().and_then(|s| s.parse().ok()).unwrap_or(40_000);
    let words = words(per_group);
    let theirs = disassemble(&mc, &words);
    let mut failures: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut compared = 0usize;
    for (&w, theirs) in words.iter().zip(theirs) {
        let insn = decode(w);
        compared += 1;
        let ours = insn.as_ref().map(Insn::to_string);
        let ok = match (&insn, &ours, &theirs) {
            (None, None, None) => true,
            (Some(i), Some(o), Some(t)) => {
                o == t || tolerated(i, o, t) || later_architecture(Some(i), t) || older_llvm(w, o, Some(t))
            }
            (Some(_), Some(o), None) => older_llvm(w, o, None),
            (None, None, Some(t)) => later_architecture(None, t),
            _ => false,
        };
        if !ok {
            let key = theirs.as_deref().or(ours.as_deref()).unwrap_or("").split(' ').next().unwrap_or("").to_string();
            let entry = failures.entry(key).or_default();
            entry.0 += 1;
            if entry.1.len() < 4 {
                entry.1.push(format!(
                    "{w:#010x}: ours {:?}, llvm-mc {:?}",
                    ours.as_deref().unwrap_or("(invalid)"),
                    theirs.as_deref().unwrap_or("(invalid)")
                ));
            }
        }
    }
    eprintln!("llvm_mc: {compared} words compared");
    if !failures.is_empty() {
        let total: usize = failures.values().map(|f| f.0).sum();
        let mut report = format!("{total} words differ from llvm-mc, by mnemonic:\n");
        for (m, (n, examples)) in &failures {
            report.push_str(&format!("  {m}: {n}\n"));
            for e in examples {
                report.push_str(&format!("    {e}\n"));
            }
        }
        panic!("{report}");
    }
}

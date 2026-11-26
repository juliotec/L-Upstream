//! The links: the game's executable and the renderer's module, each skipped when what it reads and writes is as the
//! last link left it; and the checks after them (the image's reach, the module's unresolved symbols).
//!
//! A link's record, `<output>.linkkey` beside it after a link that succeeded: the key (the command, the compiler's
//! identity, each object's key from its stamp, the contents of the other files it names), every file the linker itself
//! read as it says (the libraries found by search path, the C runtime's archives) by size and time, the linker program,
//! and the outputs (the binary and its map) by size, time and id. Any of them changed, or no record (a failed link, a
//! linker that does not list what it reads), links again. Delete the record to link again by hand.

use crate::state::Stat;
use build_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The record's format.
const RECORD: u32 = 2;

#[derive(Serialize, Deserialize, PartialEq)]
struct Record {
    record: u32,
    key: String,
    linker: Vec<(PathBuf, Stat)>,
    outputs: Vec<(PathBuf, Stat)>,
    inputs: Vec<(PathBuf, Stat)>,
}

/// What a link said, and whether it ran.
#[derive(Debug)]
pub struct Linked {
    /// It succeeded (or was not needed).
    pub ok: bool,
    /// It was not run: everything was as the last link left it.
    pub skipped: bool,
    /// Its output, both streams, without the linker's list of what it read.
    pub said: String,
}

impl Linked {
    /// The undefined symbols it named, each once, sorted.
    pub fn undefined(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.said.lines().filter(|l| l.contains("undefined symbol")).map(|l| l.trim().to_string()).collect();
        v.sort();
        v.dedup();
        v
    }
}

fn stats(paths: &[PathBuf]) -> Option<Vec<(PathBuf, Stat)>> {
    paths.iter().map(|p| Stat::of(p).map(|s| (p.clone(), s))).collect()
}

/// The objects of the command's response files, and the other existing files it names (not its outputs).
fn link_inputs(cmd: &[String], output: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut objs, mut others) = (Vec::new(), Vec::new());
    for arg in cmd {
        if let Some(rsp) = arg.strip_prefix('@').filter(|r| Path::new(r).is_file()) {
            if let Ok(text) = std::fs::read_to_string(rsp) {
                objs.extend(
                    text.lines().map(|l| l.trim().trim_matches('"')).filter(|l| !l.is_empty()).map(PathBuf::from),
                );
            }
            others.push(PathBuf::from(rsp));
            continue;
        }
        // (a file inside an option: -Xlink=/order:@<file>, --symbol-ordering-file=<file>; a whole argument too)
        let inner = arg
            .strip_prefix("-Xlink=")
            .unwrap_or(arg)
            .split_once([':', '='])
            .map(|(_, v)| v.trim_start_matches('@').to_string());
        for p in [Some(arg.clone()), inner].into_iter().flatten() {
            let path = Path::new(&p);
            if path.is_file() && path != output && !p.ends_with(".map") {
                others.push(path.to_path_buf());
                break;
            }
        }
    }
    (objs, others)
}

/// The maps the command asks for: `-Map=<file>` (lld), `/map:<file>` (lld-link, and lld's MinGW driver).
fn link_maps(cmd: &[String]) -> Vec<PathBuf> {
    cmd.iter()
        .filter_map(|a| {
            let a = a.strip_prefix("-Xlink=").unwrap_or(a);
            let lower = a.to_lowercase();
            ["--map=", "-map=", "/map:"].iter().find_map(|p| lower.starts_with(p).then(|| PathBuf::from(&a[p.len()..])))
        })
        .collect()
}

fn link_key(cmd: &[String], output: &Path, compiler_id: &str) -> Option<String> {
    let mut k = build_core::key::Key::new();
    k.text("command", &cmd.join("\0")).text("compiler", compiler_id);
    let (objs, others) = link_inputs(cmd, output);
    for obj in objs {
        let stamp = crate::compile::stamp_of(&obj);
        match std::fs::read_to_string(&stamp) {
            Ok(key) => k.text(&obj.to_string_lossy(), &key),
            Err(_) => {
                let s = Stat::of(&obj)?;
                k.text(&obj.to_string_lossy(), &format!("{} {}", s.0, s.1))
            }
        };
    }
    for p in others {
        k.part(&p.to_string_lossy(), std::fs::read(&p).ok()?.as_slice());
    }
    Some(k.hex())
}

/// What makes the linker list every file it reads, for the compiler's target (`-dumpmachine`); `None` for a target
/// this does not know (that link keeps no record and always runs).
fn link_trace(cmd: &[String]) -> Option<Vec<String>> {
    let mut probe = crate::toolchain::clang(&cmd[0]);
    probe.args(cmd.iter().filter(|a| a.starts_with("--target="))).arg("-dumpmachine");
    let out = build_core::child::output(&mut probe).ok().filter(|o| o.status.success())?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
    if t.contains("msvc") {
        Some(vec!["-Xlinker".into(), "/verbose".into()])
    } else if t.contains("mingw") || t.contains("windows") {
        Some(vec!["-Wl,--verbose".into()])
    } else if t.contains("linux") {
        Some(vec!["-Wl,--trace".into()])
    } else {
        None
    }
}

/// lld's verbose lines that are its list (files read, members taken, directives, the command its MinGW driver made).
fn is_verbose(line: &str) -> bool {
    let l = line.trim();
    let after = ["ld.lld: ", "ld.lld.exe: ", "lld-link: ", "lld-link.exe: ", "lld: ", "lld.exe: "]
        .iter()
        .find_map(|p| l.strip_prefix(p));
    after.is_some_and(|r| r.starts_with("Reading ") || r.starts_with("Loaded ") || r.starts_with("Directives: "))
        || l.starts_with("lld-link -lldmingw ")
        || l.starts_with("lld-link.exe -lldmingw ")
}

/// The files a link's own list names, and the rest of its output. An archive's member counts as its archive; a line
/// that names no file that exists is output, not the list.
fn traced_files(text: &str) -> (Vec<PathBuf>, String) {
    let (mut files, mut rest) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let s = line.trim();
        let mut name = [
            "ld.lld: Reading ",
            "ld.lld.exe: Reading ",
            "lld-link: Reading ",
            "lld-link.exe: Reading ",
            "lld: Reading ",
        ]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(s);
        // (GNU ld's -t: a library by its -l name, then the file it found)
        if name.starts_with("-l")
            && let Some((_, f)) = name.split_once(" (")
        {
            name = f.strip_suffix(')').unwrap_or(f);
        }
        let mut path = name.to_string();
        if let Some(open) = name.rfind('(').filter(|_| name.ends_with(')'))
            && Path::new(&name[..open]).is_file()
        {
            path = name[..open].to_string();
        }
        if !path.is_empty() && Path::new(&path).is_file() {
            files.push(std::path::absolute(&path).unwrap_or_else(|_| PathBuf::from(&path)));
        } else if !is_verbose(s) {
            rest.push(line);
        }
    }
    let mut r = rest.join("\n");
    if !rest.is_empty() {
        r.push('\n');
    }
    (files, r)
}

/// The linker the compiler runs for this command (`-###`'s last command's first word).
fn link_program(cmd: &[String]) -> Option<PathBuf> {
    let out = build_core::child::output(crate::toolchain::clang(&cmd[0]).args(&cmd[1..]).arg("-###")).ok()?;
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err.lines().rev().find(|l| l.trim_start().starts_with('"'))?;
    let prog = line.trim_start()[1..].split('"').next()?.to_string();
    // (Windows: clang names ld.lld without its .exe)
    [PathBuf::from(&prog), PathBuf::from(format!("{prog}.exe"))]
        .into_iter()
        .chain(crate::toolchain::which(&prog))
        .find(|p| p.is_file())
}

/// Runs a link, or nothing when everything it reads and writes is as the last successful one left it.
pub fn run(cmd: &[String], output: &Path, compiler_id: &str, log: &dyn Fn(&str)) -> Result<Linked> {
    let record_path = PathBuf::from(format!("{}.linkkey", output.display()));
    let mut outputs = vec![output.to_path_buf()];
    outputs.extend(link_maps(cmd));
    let key = link_key(cmd, output, compiler_id);
    let program = link_program(cmd);
    let old: Option<Record> = std::fs::read(&record_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    if let (Some(key), Some(program), Some(old)) = (&key, &program, &old) {
        let same = old.record == RECORD
            && &old.key == key
            && Some(&old.linker) == stats(std::slice::from_ref(program)).as_ref()
            && Some(&old.outputs) == stats(&outputs).as_ref()
            && !old.inputs.is_empty()
            && stats(&old.inputs.iter().map(|i| i.0.clone()).collect::<Vec<_>>()).as_ref() == Some(&old.inputs);
        if same {
            log(&format!(
                "  ({}: what it reads and writes is as the last link left it, not linked again)",
                output.file_name().unwrap_or_default().to_string_lossy()
            ));
            return Ok(Linked { ok: true, skipped: true, said: String::new() });
        }
    }
    build_core::fsx::remove_file(&record_path)?;
    let trace = link_trace(cmd);
    let mut full = cmd.to_vec();
    full.extend(trace.iter().flatten().cloned());
    let out = build_core::child::output(crate::toolchain::clang(&full[0]).args(&full[1..]))
        .map_err(|e| Error::tool(&cmd[0], e.to_string()))?;
    let (so, se) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let (mut read, said) = if trace.is_some() {
        let (f1, r1) = traced_files(&so);
        let (f2, r2) = traced_files(&se);
        ([f1, f2].concat(), format!("{r1}{r2}"))
    } else {
        (Vec::new(), format!("{so}{se}"))
    };
    let skip: Vec<PathBuf> = link_inputs(cmd, output)
        .0
        .iter()
        .chain(&outputs)
        .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
        .collect();
    read.retain(|f| !skip.contains(f));
    read.sort();
    read.dedup();
    if out.status.success()
        && let (Some(key), Some(program), Some(outs), Some(ins)) =
            (key, program, stats(&outputs), stats(&read).filter(|i| !i.is_empty()))
        && let Some(linker) = stats(&[program])
    {
        let rec = Record { record: RECORD, key, linker, outputs: outs, inputs: ins };
        if let Ok(bytes) = serde_json::to_vec(&rec) {
            build_core::fsx::write_atomic(&record_path, &bytes)?;
        }
    }
    Ok(Linked { ok: out.status.success(), skipped: false, said })
}

/// The objects, a quoted line each, in a response file (the command line would be too long on Windows).
pub fn response_file(path: &Path, objects: &[PathBuf]) -> Result<()> {
    let text: Vec<String> =
        objects.iter().map(|o| format!("\"{}\"", o.display().to_string().replace('\\', "/"))).collect();
    build_core::fsx::write_if_changed(path, text.join("\n").as_bytes()).map(|_| ())
}

/// The game's code reaches its data RIP-relative, a signed 32-bit offset.
pub const IMAGE_LIMIT: u64 = 1 << 31;

/// Bytes from an image's start to the end of its last section as the loader maps it: a PE's SizeOfImage, or an ELF's
/// PT_LOAD segments from the lowest address to the highest end; `None` when it is neither.
///
/// lld does not range-check x86-64 COFF's RIP-relative fixups: an -O0 build linked a 2,262 MB exe with no error, whose
/// first read of a64_dirty read 4 GB below it. So the build checks the size itself (on ELF a second check: lld does).
pub fn image_span(path: &Path) -> Option<u64> {
    let data = std::fs::read(path).ok()?;
    let u16le = |i: usize| data.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32le = |i: usize| data.get(i..i + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap_or_default()));
    let u64le = |i: usize| data.get(i..i + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap_or_default()));
    if data.starts_with(b"MZ") {
        let pe = u32le(0x3C)? as usize;
        if data.get(pe..pe + 4)? != b"PE\0\0" {
            return None;
        }
        return u32le(pe + 24 + 56).map(u64::from); // (OptionalHeader.SizeOfImage, PE32 and PE32+ alike)
    }
    if data.starts_with(b"\x7fELF") && data.get(4) == Some(&2) && data.get(5) == Some(&1) {
        let (phoff, phentsize, phnum) = (u64le(0x20)? as usize, u16le(0x36)? as usize, u16le(0x38)? as usize);
        let (mut lo, mut hi) = (u64::MAX, 0u64);
        for i in 0..phnum {
            let e = phoff.checked_add(i.checked_mul(phentsize)?)?;
            if u32le(e)? == 1 {
                let (vaddr, memsz) = (u64le(e + 0x10)?, u64le(e + 0x28)?);
                lo = lo.min(vaddr);
                hi = hi.max(vaddr.saturating_add(memsz)); // (a header's sum past 2^64: no wrap, the most)
            }
        }
        return (hi > 0).then(|| hi - lo);
    }
    None
}

/// The link's failure when the image is past what its code can address (the image is deleted then, so nothing
/// starts it).
pub fn image_too_big(exe: &Path) -> Option<String> {
    let span = image_span(exe).filter(|&s| s >= IMAGE_LIMIT)?;
    let _ = std::fs::remove_file(exe);
    Some(format!(
        "link FAILED: the game's image is {} MB, past the {} MB its code can address (x86-64 RIP-relative); the linker \
         does not check this and the game would read the wrong memory. The lifted code is too big for one image: build \
         it at -O1 (not -O0), or split the lifted code into more than one module",
        span >> 20,
        IMAGE_LIMIT >> 20
    ))
}

/// (Linux) The renderer's module is opened with RTLD_NOW, so one unresolved symbol stops it loading with no symbol
/// named; the link itself does not fail (a shared object may leave symbols to its loader). So anything still undefined
/// that is not weak, not the C or C++ runtime's (and not Tracy's C API, in a Tracy build) is a mistake: named here.
/// It has caught a host function referenced without `__attribute__((weak))`, and an `extern` of our own global inside
/// an anonymous namespace. `Ok(None)`: none, or no `nm` to ask (a guard, not a dependency).
pub fn vk_unresolved(nm: Option<&Path>, module: &Path, tracy: bool) -> Result<Option<Vec<String>>> {
    // (GNU nm on some distributions drops the symbol versions, making libc's imports look unresolved: llvm-nm first)
    let Some(nm) = nm else { return Ok(None) };
    let mut list = build_core::child::command(nm);
    list.args(["-D", "--undefined-only"]).arg(module);
    let Ok(out) = build_core::child::output(&mut list) else { return Ok(None) };
    if !out.status.success() {
        return Ok(None);
    }
    let bad: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            (w.next()? == "U").then(|| w.next().map(String::from)).flatten()
        })
        .filter(|n| !["@GLIBC", "@CXXABI", "@GLIBCXX", "@GCC"].iter().any(|v| n.contains(v)))
        .filter(|n| !(tracy && n.starts_with("___tracy_")))
        .collect();
    Ok((!bad.is_empty()).then_some(bad))
}

/// `<build>/compile_commands.json` for clangd and clang-tidy: every compile this build runs, the lifted code left out
/// (1.4 GB clangd's index would read). A failure to write it is no failed build.
pub fn write_compdb(out: &Path, entries: &[(Vec<String>, PathBuf, PathBuf)]) {
    let dir = std::env::current_dir().unwrap_or_default().display().to_string();
    let list: Vec<serde_json::Value> = entries
        .iter()
        .map(|(cmd, src, obj)| {
            serde_json::json!({"directory": dir, "file": src.display().to_string(), "output": obj.display().to_string(), "arguments": cmd})
        })
        .collect();
    if let Ok(bytes) = serde_json::to_vec_pretty(&list) {
        let _ = build_core::fsx::write_if_changed(&out.join("compile_commands.json"), &bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_linker_s_list_is_parted_from_its_output() {
        let d = crate::testdir("trace");
        let lib = d.join("libx.a");
        std::fs::write(&lib, b"!<arch>").unwrap();
        let text = format!(
            "{}\nld.lld: Reading {}\n{}(member.o)\nld.lld: error: undefined symbol: f\nld.lld: Reading /no/such\n",
            lib.display(),
            lib.display(),
            lib.display()
        );
        let (files, rest) = traced_files(&text);
        assert_eq!(files, [lib.clone(), lib.clone(), lib]);
        assert_eq!(rest, "ld.lld: error: undefined symbol: f\n");
        let l = Linked {
            ok: false,
            skipped: false,
            said: "ld.lld: error: undefined symbol: f\n  x undefined symbol: g\n".into(),
        };
        assert_eq!(l.undefined().len(), 2);
    }

    #[test]
    fn maps_and_named_files() {
        let cmd: Vec<String> =
            ["clang", "-Xlinker", "-Map=/o/x.map", "-Xlinker", "-Xlink=/map:/o/y.map"].map(String::from).to_vec();
        assert_eq!(link_maps(&cmd), [PathBuf::from("/o/x.map"), PathBuf::from("/o/y.map")]);
    }
}

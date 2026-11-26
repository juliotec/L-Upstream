//! The tools the compile runs and what it builds for: clang and
//! clang++ (llvm-mingw's for a Windows game when `LONGSHOT_MINGW` names it; else `LONGSHOT_LLVM`'s, the builder's
//! tools/llvm; else an emsdk checkout's, `EMSDK`; else the PATH's), the target and the link's flags, the Linux sysroot,
//! and the CPU the game is built for. A player's build gets the compiler, the sysroot and the libraries from the
//! builder's own tools (build-driver's tools/mod.rs), never from the PC's variables.
//!
//! The builder's llvm has no aliases (no links in its bundle): `bin/clang` is the compiler, C++ is it with
//! `--driver-mode=g++` ([`Toolchain::cxx_mode`]), and `bin/ld.lld` the linker clang's `-fuse-ld=lld` runs.

use crate::options::Settings;
use build_core::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The variables clang and its linker read that change what they make without showing in a unit's flags, so in no key:
/// `CCC_OVERRIDE_OPTIONS` edits the command line itself, `COMPILER_PATH` and `SDKROOT` choose the programs and the
/// system it builds against, `LIBRARY_PATH`, `CPATH`, `C_INCLUDE_PATH` and `CPLUS_INCLUDE_PATH` add library and header
/// folders. Left out of every run of the compiler ([`clang`]): an object is made from its flags alone.
pub const UNKEYED_ENV: [&str; 7] = [
    "CCC_OVERRIDE_OPTIONS",
    "COMPILER_PATH",
    "LIBRARY_PATH",
    "CPATH",
    "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH",
    "SDKROOT",
];

/// A command running the compiler (a compile, a link, a probe) at `program`: tied to the builder
/// ([`build_core::child`]), without the variables no key holds ([`UNKEYED_ENV`]).
pub fn clang(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = build_core::child::command(program);
    for v in UNKEYED_ENV {
        c.env_remove(v);
    }
    c
}

/// The tools and the target.
#[derive(Clone, Debug)]
pub struct Toolchain {
    /// The game is built for Windows.
    pub windows: bool,
    /// llvm-mingw's folder for a Windows game, if that is the route (else the MSVC target, Windows only).
    pub mingw: Option<PathBuf>,
    /// The C compiler.
    pub cc: PathBuf,
    /// The C++ compiler.
    pub cxx: PathBuf,
    /// What makes `cxx` the C++ driver when it is clang itself (`--driver-mode=g++`: no `clang++` beside it), else
    /// nothing: every link run with `cxx` gives these first.
    pub cxx_mode: Vec<String>,
    /// The target's flags for every compile and link (`native_target`, and the sysroot on Linux).
    pub target: Vec<String>,
    /// The bare target flags, without the sysroot (what a Windows link takes).
    pub native_target: Vec<String>,
    /// What every Windows link adds (`native_link`).
    pub native_link: Vec<String>,
    /// The game's CPU flags: `-march=...`, no contraction, and the CPU `native` stood for.
    pub cpu: Vec<String>,
    /// The third-party code's CPU flags: x86-64-v3 on every CPU that has it, so the release's prebuilt set fits.
    pub third_party_cpu: Vec<String>,
    /// What `-march=native` is on this PC (`<cpu>_<tune>_<features' hash>`), when the game is built for it.
    pub native_cpu: Option<String>,
    /// `llvm-nm` (or `nm`), for the renderer's symbol check.
    pub nm: Option<PathBuf>,
}

/// A program on the PATH (with Windows' `.exe`).
pub fn which(tool: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".bat", ""] } else { &[""] };
    std::env::split_paths(&path)
        .flat_map(|d| exts.iter().map(move |e| d.join(format!("{tool}{e}"))))
        .find(|p| p.is_file())
}

/// Where a tool is: llvm-mingw's for a Windows game, `LONGSHOT_LLVM`'s `bin`, `EMSDK`'s `upstream/bin`, the PATH's;
/// else where the first named of those would be (a path that is not there, for the message).
pub fn find(tool: &str, s: &Settings) -> PathBuf {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    if let Some(m) = &s.mingw {
        let p = m.join("bin").join(format!("{}{exe}", if tool == "lld" { "ld.lld" } else { tool }));
        if p.is_file() {
            return p;
        }
    }
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".bat"] } else { &[""] };
    let folders: Vec<PathBuf> =
        s.llvm.iter().map(|l| l.join("bin")).chain([s.emsdk.join("upstream").join("bin")]).collect();
    // (the builder's llvm names lld ld.lld only)
    let names: &[&str] = if tool == "lld" { &["lld", "ld.lld"] } else { &[tool] };
    for folder in &folders {
        for name in names {
            for e in exts {
                let p = folder.join(format!("{name}{e}"));
                if p.is_file() {
                    return p;
                }
            }
        }
    }
    which(tool).unwrap_or_else(|| folders[0].join(format!("{tool}{}", exts[0])))
}

/// `path`, or the player told how to get it.
pub fn need(path: PathBuf, s: &Settings) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path);
    }
    let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Err(Error::player(
        "compiler_missing",
        format!(
            "{} not found: the builder's tools are incomplete (its tools/ folder), or a developer's LONGSHOT_LLVM names \
             an LLVM release's folder (now {}), EMSDK an emsdk checkout (now {}), or {name} is on PATH",
            path.display(),
            s.llvm.as_deref().map_or("unset".into(), |p| p.display().to_string()),
            s.emsdk.display()
        ),
    ))
}

/// The CPU levels the game may be built for, by the features they need.
const V3: &[&str] = &["avx2", "fma", "bmi2", "movbe"];
const V2: &[&str] = &["sse4.2", "popcnt", "ssse3"];

/// What clang makes of `flags` (a `-march`): its target CPU, tune CPU and features, from `-###`.
fn probe(cc: &Path, flags: &[String]) -> Option<(String, String, Vec<String>)> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut ask = clang(cc);
    ask.args(flags).args(["-###", "-x", "c", "-c", null, "-o", null]);
    let out = build_core::child::output(&mut ask).ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    let words: Vec<&str> = text.split_whitespace().map(|w| w.trim_matches('"')).collect();
    let (mut cpu, mut tune, mut feats) = (String::new(), String::new(), Vec::new());
    for w in words.windows(2) {
        match w[0] {
            "-target-cpu" => cpu = w[1].to_string(),
            "-tune-cpu" => tune = w[1].to_string(),
            "-target-feature" => feats.push(w[1].to_string()),
            _ => {}
        }
    }
    (!cpu.is_empty() && cpu != "native").then_some((cpu, tune, feats))
}

/// `-march=native`'s name for the keys and the crash window (`host_cpu.native_cpu`): `<cpu>_<tune>_<first 10 of the
/// sorted features' SHA-1>`.
fn native_name(cpu: &str, tune: &str, feats: &[String]) -> String {
    let mut f = feats.to_vec();
    f.sort();
    let h = build_core::key::sha1_hex(f.join(" ").as_bytes());
    format!("{cpu}_{}_{}", if tune.is_empty() { cpu } else { tune }, &h[..10])
}

/// The level this PC's CPU runs, from `/proc/cpuinfo` where clang could not tell (`host_cpu.host_march`).
fn level_of_cpuinfo() -> &'static str {
    let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") else { return "x86-64-v3" };
    let Some(line) = text.lines().find(|l| l.starts_with("flags")) else { return "x86-64-v3" };
    let flags: Vec<&str> = line.split_whitespace().collect();
    let has = |want: &[&str]| want.iter().all(|w| flags.contains(&w.replace('.', "_").as_str()));
    if has(V3) {
        "x86-64-v3"
    } else if has(V2) {
        "x86-64-v2"
    } else {
        "x86-64"
    }
}

impl Toolchain {
    /// The toolchain the settings choose, on this PC. `log` gets the lines the build log says of it.
    pub fn find(s: &Settings, work: &Path, log: &dyn Fn(&str)) -> Result<Toolchain> {
        let cc = need(find("clang", s), s)?;
        // (no clang++ beside clang, as in the builder's llvm: clang itself, as the C++ driver)
        let beside = cc.with_file_name(format!("clang++{}", if cfg!(windows) { ".exe" } else { "" }));
        let (cxx, cxx_mode) = if s.mingw.is_none() && cc.parent().is_some() && !beside.exists() {
            (cc.clone(), vec!["--driver-mode=g++".to_string()])
        } else {
            (need(find("clang++", s), s)?, Vec::new())
        };
        let mingw = s.mingw.clone().filter(|_| s.windows);
        let native_target: Vec<String> = if mingw.is_some() {
            // (-fms-extensions: __try/__except and __declspec, which clang takes on this target too)
            vec!["--target=x86_64-w64-mingw32".into(), "-fms-extensions".into(), "-D_CRT_SECURE_NO_WARNINGS".into()]
        } else if cfg!(windows) {
            vec!["--target=x86_64-pc-windows-msvc".into(), "-fms-extensions".into(), "-D_CRT_SECURE_NO_WARNINGS".into()]
        } else {
            vec![]
        };
        let mut target = native_target.clone();
        if !s.windows
            && let Some(root) = &s.sysroot
        {
            target.push(format!("--sysroot={}", root.display()));
        }
        let native_link = if mingw.is_some() {
            // (its own lld; the C++ runtime and winpthread linked in, so no DLLs of the toolchain go beside the game.
            // /nodefaultlib: libraries built with Microsoft's compiler (the CPM cache's FFmpeg) ask for its C runtime;
            // the MinGW one stands in, with host/mingw_msvc_shims.c for what it lacks)
            let mut l = vec!["-fuse-ld=lld".to_string(), "-static".to_string(), reproducible(true)];
            l.extend(["libcmt", "oldnames", "libvcruntime", "msvcrt"].map(|x| format!("-Wl,-Xlink=/nodefaultlib:{x}")));
            l
        } else if cfg!(windows) {
            // (-fuse-ld=lld through -B: clang ignores --ld-path for the MSVC target and ran Microsoft's link.exe.
            // oldnames: recent Windows SDKs no longer ship it, issue 439)
            let dir = lld_link(s, work)?;
            vec![
                "-fuse-ld=lld".into(),
                format!("-B{}", dir.display()),
                "-Wl,/nodefaultlib:oldnames".into(),
                reproducible(false),
            ]
        } else {
            vec![]
        };
        let (cpu, third_party_cpu, native_cpu) = cpu_flags(&cc, &target, s, log)?;
        let nm = which("llvm-nm").or_else(|| which("nm"));
        Ok(Toolchain {
            windows: s.windows,
            mingw,
            cc,
            cxx,
            cxx_mode,
            target,
            native_target,
            native_link,
            cpu,
            third_party_cpu,
            native_cpu,
            nm,
        })
    }

    /// A link.exe-style option (`/map:x`): as it is for the MSVC target, through lld's MinGW driver for llvm-mingw.
    pub fn link_opt(&self, opt: &str) -> String {
        if self.mingw.is_some() { format!("-Wl,-Xlink={opt}") } else { format!("-Wl,{opt}") }
    }

    /// Linker options, each an argument of its own after `-Xlinker`. The ones that name a file are in the work folder,
    /// which the player chooses, and the compiler splits `-Wl,` at every comma: a comma in that folder's path cut the
    /// map's path in two and stopped the link.
    pub fn linker(&self, opts: &[String]) -> Vec<String> {
        opts.iter()
            .flat_map(|o| {
                let o = if self.mingw.is_some() { format!("-Xlink={o}") } else { o.clone() };
                ["-Xlinker".to_string(), o]
            })
            .collect()
    }
}

/// lld's /Brepro for a Windows link (llvm-mingw's driver, `mingw`, takes it through -Xlink): the image's time stamp a
/// hash of its contents, not the time of the link, so a link of the same inputs makes the same bytes. Without it every
/// relink (a game folder typed in another case, another --jobs) gave the renderer other bytes, and the shader
/// preparation, keyed by its SHA-1, ran again: minutes, with nothing changed.
fn reproducible(mingw: bool) -> String {
    if mingw { "-Wl,-Xlink=/Brepro".into() } else { "-Wl,/Brepro".into() }
}

/// (MSVC route) lld under the name lld-link (lld picks its COFF linker from its own file name), made on first use in
/// the work folder's tools: the folder to give clang's `-B`.
fn lld_link(s: &Settings, work: &Path) -> Result<PathBuf> {
    let dir = work.join("tools");
    let lld = need(find("lld", s), s)?;
    place_lld_link(&lld, &dir)?;
    Ok(dir)
}

/// `lld` as `<dir>/lld-link.exe`, checked against it every build (its SHA-1): a copy that is not lld's bytes (another
/// tools release, or a file put there) is placed again, never run.
fn place_lld_link(lld: &Path, dir: &Path) -> Result<()> {
    let path = dir.join("lld-link.exe");
    // (a hard link of lld is lld: nothing to hash)
    if path.is_file()
        && (build_core::fsx::same_file(&path, lld) || crate::key::file_sha1(&path)? == crate::key::file_sha1(lld)?)
    {
        return Ok(());
    }
    build_core::fsx::link_or_copy(lld, &path)?;
    Ok(())
}

/// The CPU flags, the game's and the third-party code's. x86-64-v3 or better builds for this PC's own CPU
/// (`-march=native`, its name in `LONGSHOT_NATIVE_CPU`, which the crash window shows and the key holds); a CPU without
/// v3's features gets x86-64-v2 or x86-64 (a CPU without AVX2 would stop at the first AVX2 instruction). `RX_MARCH`
/// chooses instead. No contraction: a C expression is rounded as written (the exact bodies and FPX1's proofs assume it).
type CpuFlags = (Vec<String>, Vec<String>, Option<String>);

fn cpu_flags(cc: &Path, target: &[String], s: &Settings, log: &dyn Fn(&str)) -> Result<CpuFlags> {
    let nofma = "-ffp-contract=off".to_string();
    let with = |m: &str| vec![format!("-march={m}"), nofma.clone()];
    let native = |extra: &[String]| -> Option<(String, Vec<String>)> {
        let mut f = target.to_vec();
        f.extend(extra.iter().cloned());
        f.extend(with("native"));
        probe(cc, &f).map(|(c, t, feats)| (native_name(&c, &t, &feats), feats))
    };
    let (march, name) = match s.march.as_deref() {
        Some("native") => match native(&[]) {
            Some((n, _)) => ("native".to_string(), Some(n)),
            None => {
                return Err(Error::player(
                    "cpu_unknown",
                    "clang does not say what -march=native is on this PC: set RX_MARCH to a CPU level",
                ));
            }
        },
        Some(m) => {
            if m != "x86-64-v3" {
                log(&format!("the game is built for {m} (RX_MARCH)"));
            }
            (m.to_string(), None)
        }
        None => match native(&[]) {
            Some((n, feats)) => {
                let has = |want: &[&str]| want.iter().all(|w| feats.iter().any(|f| f == &format!("+{w}")));
                if has(V3) {
                    ("native".to_string(), Some(n))
                } else {
                    let m = if has(V2) { "x86-64-v2" } else { "x86-64" };
                    log(&format!("this CPU has no AVX2: the game is built for {m} (a little slower)"));
                    (m.to_string(), None)
                }
            }
            None => {
                let m = level_of_cpuinfo();
                if m != "x86-64-v3" {
                    log(&format!("this CPU has no AVX2: the game is built for {m} (a little slower)"));
                }
                (m.to_string(), None)
            }
        },
    };
    let mut cpu = with(&march);
    if let Some(n) = &name {
        log(&format!("the game is built for this PC's CPU ({n})"));
        cpu.push(format!("-DLONGSHOT_NATIVE_CPU={n}"));
    }
    let third =
        if s.march.is_none() && (march == "native" || march == "x86-64-v3") { with("x86-64-v3") } else { cpu.clone() };
    Ok((cpu, third, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiler_runs_without_the_variables_no_key_holds() {
        let c = clang("clang");
        let envs: Vec<(String, Option<String>)> = c
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned())))
            .collect();
        for v in UNKEYED_ENV {
            assert!(envs.contains(&(v.to_string(), None)), "{v} left in: {envs:?}");
        }
    }

    #[test]
    fn lld_link_is_lld_s_bytes_every_build() {
        let d = std::env::temp_dir().join(format!("build-compile-lld-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let lld = d.join("lld");
        std::fs::write(&lld, b"lld 1").unwrap();
        place_lld_link(&lld, &d.join("tools")).unwrap();
        // (a file put at its name, or the tools moved on: lld's bytes again; a new file, as one written in place
        // through a hard link would be lld itself)
        std::fs::remove_file(d.join("tools/lld-link.exe")).unwrap();
        std::fs::write(d.join("tools/lld-link.exe"), b"something else").unwrap();
        place_lld_link(&lld, &d.join("tools")).unwrap();
        assert_eq!(std::fs::read(d.join("tools/lld-link.exe")).unwrap(), b"lld 1");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// A Windows image linked twice from the same object, a second apart: the same bytes with [`reproducible`], other
    /// bytes (the link's time) without. With the PC's clang and lld, as tests/compile_link.rs (none: passed over only
    /// with LONGSHOT_NO_CLANG=1).
    #[test]
    fn a_windows_link_of_the_same_inputs_makes_the_same_bytes() {
        let Some(cc) = which("clang") else {
            assert_eq!(std::env::var("LONGSHOT_NO_CLANG").as_deref(), Ok("1"), "no clang to link with");
            return;
        };
        let d = crate::testdir("toolchain-brepro");
        std::fs::write(d.join("m.c"), "int mainCRTStartup(void) { return 0; }\n").unwrap();
        let link = |out: &str, extra: &[String]| {
            let status = clang(&cc)
                .args(["--target=x86_64-pc-windows-msvc", "-fuse-ld=lld", "-nostdlib", "-O1", "-o"])
                .arg(d.join(out))
                .arg(d.join("m.c"))
                .args(["-Wl,/entry:mainCRTStartup", "-Wl,/subsystem:console"])
                .args(extra)
                .status()
                .unwrap();
            assert!(status.success(), "{out}");
            std::fs::read(d.join(out)).unwrap()
        };
        let flag = [reproducible(false)];
        let (a, plain_a) = (link("a.exe", &flag), link("pa.exe", &[]));
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let (b, plain_b) = (link("b.exe", &flag), link("pb.exe", &[]));
        assert_eq!(a, b, "the same inputs, the same bytes");
        assert_ne!(plain_a, plain_b, "without it, the link's time is in the image");
        assert_eq!(reproducible(true), "-Wl,-Xlink=/Brepro", "llvm-mingw's driver: the COFF option through -Xlink");
    }

    #[test]
    fn native_s_name_is_the_cpu_and_the_features_hash() {
        let n = native_name("znver4", "", &["+b".into(), "+a".into()]);
        assert_eq!(n, format!("znver4_znver4_{}", &build_core::key::sha1_hex(b"+a +b")[..10]));
    }

    #[test]
    fn a_missing_compiler_says_what_to_set() {
        let s = Settings::from_lookup(|n| (n == "EMSDK").then(|| "/nowhere".to_string()));
        let e = need(PathBuf::from("/nowhere/upstream/bin/clang"), &s).unwrap_err();
        assert_eq!(e.code(), "compiler_missing");
        assert!(e.to_string().starts_with("/nowhere/upstream/bin/clang not found: the builder's tools are incomplete"));
        assert!(e.to_string().contains("LONGSHOT_LLVM names an LLVM release's folder (now unset), EMSDK an emsdk"));
    }

    #[test]
    fn the_builder_s_llvm_comes_before_emsdk_and_the_path() {
        let d = crate::testdir("toolchain-llvm");
        let bin = d.join("llvm/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for f in ["clang", "ld.lld"] {
            std::fs::write(bin.join(f), "").unwrap();
        }
        let llvm = d.join("llvm").display().to_string();
        let s = Settings::from_lookup(|n| match n {
            "LONGSHOT_LLVM" => Some(llvm.clone()),
            "EMSDK" => Some("/nowhere".into()),
            _ => None,
        });
        assert_eq!(find("clang", &s), bin.join("clang"));
        assert_eq!(find("lld", &s), bin.join("ld.lld"));
    }
}

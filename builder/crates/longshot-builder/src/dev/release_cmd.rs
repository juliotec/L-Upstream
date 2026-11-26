//! `longshot-builder dev release`: a release's packaging (docs/dev/reference/releases.md; CI's build-linux.yml,
//! build-windows.yml and release.yml run it with build-linux.yml's own longshot-builder, before its pack is attached).
//!
//! ```text
//! sources --out <pack> --version <name> [--root <checkout>] [--third-party-objects <folder>]
//!         [--add <path in the tree>=<file> ...] [--tool <name>=<folder> ...] [--tools <folder> ...] [--reuse <pack>]
//!                      the pack a release's longshot-builder carries (build_driver::sources' writer, its format there):
//!                      the checkout's tracked tree, VERSION, the third-party objects, the files added (RenHDrum's
//!                      library), each tool (--tools: each folder of a bundle's tools/), every member compressed
//!                      apart by the zstd program, several at once (one zstd each, as many as the PC's threads), so the
//!                      same files give the same bytes. --reuse: the last release's pack (last-objects keeps it, its
//!                      signature checked): each member of it whose key is this one's (its raw bytes, the zstd program
//!                      and its arguments) is taken as it is, not compressed again; a pack that can't be read is said,
//!                      and everything compressed
//! unpack <pack> <folder>
//!                      a pack's files written into a folder, checked, as the program reads them
//! attach <program> <pack> --out <file>
//!                      the program a release ships: <program> (a linked longshot-builder that carries none) with the
//!                      pack appended and its trailer after it (build_driver::sources' format), read back and checked
//! dry-run <program> --work <folder> [--root <tree>] [--cache <folder>] [--clean]
//!                      a player's build's dry run over a tree, with and without AMD FSR 4, checked: every file it
//!                      compiles there, no unit missing, every third-party object taken from the tools' set.
//!                      <program>: a release's longshot-builder (its own tree: --clean, nothing of this environment
//!                      but PATH and HOME, first its `sources`, timed), or one that carries nothing with --root
//! last-objects --version <tag> --platform <p> --out <folder> [--api <url>] [--repo <o/n>]
//!                      the third-party objects of the channel's newest release with the platform's files, its signed
//!                      SHA256SUMS checked first (release_objects.rs), into <folder>/objects/third_party_objects: the
//!                      set `compile --third-party-objects` takes the objects of this build's keys from
//!                      (LONGSHOT_THIRD_PARTY_OBJECTS). Nothing there when none could be had: said, never a failure
//! fetch-bases --platform <p> --channel nightly|stable --out <folder> [--not <tag>] [--api <url>] [--repo <o/n>]
//!             [--keep N]
//!                      the last releases' full files of a channel (a nightly's: and the latest stable's), to make
//!                      deltas from (release_deltas.rs): through
//!                      FORGEJO_UPLOAD_API's server first when it is set, as Tooling's release.py downloads, then --api's
//! deltas <full file> --version <tag> --channel nightly|stable --platform <p> --out <folder> [--bases <folder>]
//!        [--stamps <json file>]
//!                      the deltas from earlier full files, and the release's manifest
//! files <full file> --version <tag> --platform <p> --work <folder> [--runner <program>] [--api <url>] [--repo <o/n>]
//!                      a release's stamps, deltas and manifest beside its full file (fetch-bases, then deltas)
//! recomplauncher <full file> [--out <folder>] [--root <checkout>]
//!                      RecompLauncher's release file: the full file as Longshot.exe (Windows, made a windowed program)
//!                      or Longshot.x86_64 (Linux), the program players start (play.rs), and the checkout's
//!                      recomplauncher.json, zipped, stored, the same files giving the same zip
//! ```
//!
//! `--root` defaults to the checkout this runs in (the first folder at or above it with a tools.lock). Exit status 1
//! when something is wrong, 2 for a command line it does not take.

use super::release_deltas::{self as rd, Net};
use build_driver::sources;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: longshot-builder dev release sources --out <pack> --version <name> [--root <checkout>]
                         [--third-party-objects <folder>] [--add <path>=<file> ...] [--tool <name>=<folder> ...]
                         [--tools <folder> ...] [--reuse <pack>]
       longshot-builder dev release unpack <pack> <folder>
       longshot-builder dev release attach <program> <pack> --out <file>
       longshot-builder dev release dry-run <program> --work <folder> [--root <tree>] [--cache <folder>] [--clean]
       longshot-builder dev release last-objects --version <tag> --platform <p> --out <folder> [--api <url>]
                         [--repo <owner/name>]
       longshot-builder dev release fetch-bases --platform <p> --channel nightly|stable --out <folder> [--not <tag>]
                         [--api <url>] [--repo <owner/name>] [--keep N]
       longshot-builder dev release deltas <full file> --version <tag> --channel nightly|stable --platform <p>
                         --out <folder> [--bases <folder>] [--stamps <json file>]
       longshot-builder dev release files <full file> --version <tag> --platform <p> --work <folder>
                         [--runner <program>] [--api <url>] [--repo <owner/name>]
       longshot-builder dev release recomplauncher <full file> [--out <folder>] [--root <checkout>]";

/// Each command: its words after the command, the options it must have, the others it takes (`--clean` a flag).
const COMMANDS: [(&str, usize, &[&str], &[&str]); 9] = [
    (
        "sources",
        0,
        &["--out", "--version"],
        &["--root", "--third-party-objects", "--add", "--tool", "--tools", "--reuse"],
    ),
    ("unpack", 2, &[], &[]),
    ("attach", 2, &["--out"], &[]),
    ("dry-run", 1, &["--work"], &["--root", "--cache", "--clean"]),
    ("last-objects", 0, &["--version", "--platform", "--out"], &["--api", "--repo"]),
    ("fetch-bases", 0, &["--platform", "--channel", "--out"], &["--not", "--api", "--repo", "--keep"]),
    ("deltas", 1, &["--version", "--channel", "--platform", "--out"], &["--bases", "--stamps"]),
    ("files", 1, &["--version", "--platform", "--work"], &["--runner", "--api", "--repo"]),
    ("recomplauncher", 1, &[], &["--out", "--root"]),
];

/// The command line after `release`: the command, its words, each option's values (in order), `--clean`.
#[derive(Debug, Default)]
struct Args {
    command: String,
    words: Vec<String>,
    opts: BTreeMap<String, Vec<String>>,
    clean: bool,
}

impl Args {
    fn one(&self, name: &str) -> Option<&str> {
        self.opts.get(name).and_then(|v| v.last()).map(String::as_str)
    }

    fn all(&self, name: &str) -> &[String] {
        self.opts.get(name).map_or(&[], Vec::as_slice)
    }
}

fn parse(args: &[String]) -> Result<Args, String> {
    let command = args.first().ok_or("a command is needed")?;
    let &(_, words, required, others) =
        COMMANDS.iter().find(|c| c.0 == command).ok_or_else(|| format!("no command {command:?}"))?;
    let mut a = Args { command: command.clone(), ..Args::default() };
    let mut it = args[1..].iter();
    while let Some(arg) = it.next() {
        if arg == "--clean" && others.contains(&"--clean") {
            a.clean = true;
        } else if arg.starts_with("--") {
            if !required.contains(&arg.as_str()) && !others.contains(&arg.as_str()) {
                return Err(format!("{command} takes no {arg}"));
            }
            let v = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
            a.opts.entry(arg.clone()).or_default().push(v.clone());
        } else {
            a.words.push(arg.clone());
        }
    }
    if a.words.len() != words {
        return Err(format!("{command} takes {words} file names, not {}", a.words.len()));
    }
    if let Some(r) = required.iter().find(|r| !a.opts.contains_key(**r)) {
        return Err(format!("{command} needs {r}"));
    }
    if let Some(p) = a.one("--platform").filter(|p| !rd::PLATFORMS.iter().any(|(q, _)| q == p)) {
        return Err(format!("--platform {p}: linux-x64 or windows-x64"));
    }
    if let Some(c) = a.one("--channel").filter(|c| !["nightly", "stable"].contains(c)) {
        return Err(format!("--channel {c}: nightly or stable"));
    }
    if a.one("--keep").is_some_and(|k| k.parse::<usize>().is_err()) {
        return Err("--keep is a number".into());
    }
    Ok(a)
}

/// The checkout: `--root`, else the first folder at or above this one with a tools.lock.
fn checkout(given: Option<&str>) -> Result<PathBuf, String> {
    if let Some(r) = given {
        return std::path::absolute(r).map_err(|e| e.to_string());
    }
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    here.ancestors()
        .find(|d| d.join("tools.lock").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| "no checkout (tools.lock) at or above this folder: name one with --root".into())
}

/// A line of what it does; a CI annotation (`::warning::...`) as it is, where Forgejo reads it.
fn say(s: String) {
    if s.starts_with("::") {
        println!("{s}");
    } else {
        println!("longshot-builder dev release: {s}");
    }
}

/// FORGEJO_UPLOAD_API (RELEASE_UPLOAD_API in CI: the server past Cloudflare), when set: the bases are fetched through it
/// first (release_deltas.rs's Bases::mirror).
fn mirror() -> Option<String> {
    std::env::var("FORGEJO_UPLOAD_API").ok().map(|m| m.trim().to_string()).filter(|m| !m.is_empty())
}

fn sources_cmd(a: &Args) -> Result<(), String> {
    let root = checkout(a.one("--root"))?;
    let present: Vec<&str> = sources::TREE.iter().copied().filter(|p| root.join(p).exists()).collect();
    let tracked = sources::tracked(&root, &present)?;
    let objects = a.one("--third-party-objects").map(Path::new);
    let tree = sources::tree_entries(&root, &tracked, a.one("--version").unwrap_or(""), objects, a.all("--add"))?;
    let mut tools = Vec::new();
    for spec in a.all("--tool") {
        match spec.split_once('=') {
            Some((name, folder)) if !name.is_empty() && Path::new(folder).is_dir() => {
                tools.push((name.to_string(), PathBuf::from(folder)));
            }
            _ => return Err(format!("--tool {spec}: not <name>=<folder>")),
        }
    }
    for bundle in a.all("--tools") {
        let mut names: Vec<String> = std::fs::read_dir(bundle)
            .map_err(|e| format!("{bundle}: {e}"))?
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        tools.extend(names.into_iter().map(|n| (n.clone(), Path::new(bundle).join(n))));
    }
    let out = Path::new(a.one("--out").unwrap_or(""));
    let reuse = match a.one("--reuse") {
        None => Vec::new(),
        Some(p) => match std::fs::read(p)
            .map_err(|e| e.to_string())
            .and_then(|b| sources::reusable(&b).map_err(|e| e.to_string()))
        {
            Ok(m) => m,
            Err(e) if !Path::new(p).exists() => {
                say(format!("no last pack at {p} ({e}): every member is compressed"));
                Vec::new()
            }
            Err(e) => {
                say(format!("::warning::the last pack {p} could not be read ({e}): every member is compressed"));
                Vec::new()
            }
        },
    };
    let jobs = std::thread::available_parallelism().map_or(1, usize::from);
    let p = sources::Packing {
        zstd: &sources::compress,
        settings: Some(sources::release_settings()?),
        reuse: &reuse,
        jobs,
    };
    let started = std::time::Instant::now();
    let w = sources::write_with(out, &tree, &tools, &p)?;
    say(format!(
        "{}: {} members, {} files, {} MB, {} MB compressed, sha256 {}; {} members ({} MB) taken from the last pack, the \
         rest compressed {jobs} at once, in {:.1} s",
        out.display(),
        w.members,
        w.files,
        w.raw >> 20,
        w.size >> 20,
        w.sha256,
        w.reused,
        w.reused_raw >> 20,
        started.elapsed().as_secs_f64()
    ));
    Ok(())
}

fn unpack_cmd(a: &Args) -> Result<(), String> {
    let pack = std::fs::read(&a.words[0]).map_err(|e| format!("{}: {e}", a.words[0]))?;
    let n = sources::unpack_into(&pack, Path::new(&a.words[1])).map_err(|e| e.to_string())?;
    say(format!("{n} files into {}", a.words[1]));
    Ok(())
}

/// `attach`: the program with the pack appended, written through `<out>.new` (executable), then read back from the
/// file as the program reads itself: its pack found by its trailer, its SHA-256 and its table checked.
fn attach_cmd(a: &Args) -> Result<(), String> {
    let (program, pack) = (Path::new(&a.words[0]), Path::new(&a.words[1]));
    let out = Path::new(a.one("--out").unwrap_or(""));
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let bytes = sources::attach(&read(program)?, &read(pack)?).map_err(|e| format!("{}: {e}", program.display()))?;
    if let Some(dir) = std::path::absolute(out).ok().as_deref().and_then(Path::parent) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut new = out.as_os_str().to_owned();
    new.push(".new");
    let new = PathBuf::from(new);
    std::fs::write(&new, &bytes).map_err(|e| format!("{}: {e}", new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", new.display()))?;
    }
    std::fs::rename(&new, out).map_err(|e| format!("{}: {e}", out.display()))?;
    let back = read(out)?;
    let carried = sources::verify(&back).map_err(|e| format!("{}: {e}", out.display()))?;
    let release =
        sources::carried_release(carried).map_or_else(|| "no VERSION".into(), |(r, h)| format!("{r}, source {h}"));
    say(format!(
        "{}: {} bytes, the pack's {} MB at {} ({release})",
        out.display(),
        back.len(),
        carried.len() >> 20,
        back.len() - carried.len() - sources::TRAILER_LEN
    ));
    Ok(())
}

/// The dry run's line `dry run: <N> files to compile besides the lift's, 0 missing`, N above 0.
fn files_line(out: &str) -> bool {
    out.match_indices("dry run: ").any(|(i, m)| {
        let rest = &out[i + m.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0
            && !rest.starts_with('0')
            && rest[digits..].starts_with(" files to compile besides the lift's, 0 missing")
    })
}

/// The dry run's first line `dry run: the third-party objects: <A> of <B> in the tools' set`: (A, B).
fn objects_line(out: &str) -> Option<(String, String)> {
    const HEAD: &str = "dry run: the third-party objects: ";
    out.match_indices(HEAD).find_map(|(i, _)| {
        let rest = &out[i + HEAD.len()..];
        let n = rest.bytes().take_while(u8::is_ascii_digit).count();
        let after = rest[n..].strip_prefix(" of ")?;
        let m = after.bytes().take_while(u8::is_ascii_digit).count();
        (n > 0 && m > 0 && after[m..].starts_with(" in the tools' set"))
            .then(|| (rest[..n].to_string(), after[..m].to_string()))
    })
}

/// The dry run's line about the game's Rust (build-compile), printed when the tree it reads has it.
const RUST_LINE: &str = "dry run: the game's Rust: ";
/// That line when the tree carries a prebuilt `liblongshot_rs.a` of its own Rust sources.
const RUST_PREBUILT: &str = "dry run: the game's Rust: prebuilt for its sources";

/// What a dry run's two outputs (without and with AMD FSR 4) must show; the first thing wrong. The game's Rust
/// (`longshot-rs`) must be prebuilt for its sources, or a player's build would need cargo: in both runs when either
/// speaks of it (the tree the dry run read has it), or when `rust` (the checkout it runs in has it).
fn check_dry_runs(none: &str, fsr4: &str, rust: bool) -> Result<(), String> {
    let rust = rust || none.contains(RUST_LINE) || fsr4.contains(RUST_LINE);
    for (up, out) in [("none", none), ("fsr4", fsr4)] {
        if rust && !out.contains(RUST_PREBUILT) {
            return Err(format!(
                "the dry run ({up}): no prebuilt liblongshot_rs.a of the tree's Rust sources: a player's build would need cargo"
            ));
        }
        if !files_line(out) {
            return Err(format!("the dry run ({up}): files missing, or none to compile"));
        }
        match objects_line(out) {
            Some((a, b)) if a == b && a != "0" => {}
            _ => return Err(format!("the dry run ({up}): a third-party object a player's build would compile")),
        }
    }
    if !fsr4.contains("with AMD FSR 4") || none.contains("with AMD FSR 4") {
        return Err("the dry run: AMD FSR 4 not where it is asked for, or where it is not".into());
    }
    Ok(())
}

/// The environment of a `--clean` dry run: a player's PC's bare one, nothing of a developer's set. Linux: a system
/// PATH and HOME. Windows: what a program can't run without (`SystemRoot`, `windir`, `SystemDrive`, `TEMP`, `TMP`,
/// `ComSpec`), the folders the builder's cache and the vendors' files are found by (`LOCALAPPDATA`, `USERPROFILE`,
/// `APPDATA`), and Windows' own PATH (its System32 first: curl, the DLLs).
fn clean_env(
    windows: bool,
    get: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<(&'static str, std::ffi::OsString)> {
    if !windows {
        return vec![("PATH", "/usr/bin:/bin".into()), ("HOME", get("HOME").unwrap_or_else(|| "/tmp".into()))];
    }
    let mut env: Vec<(&'static str, std::ffi::OsString)> = Vec::new();
    for k in ["SystemRoot", "windir", "SystemDrive", "TEMP", "TMP", "ComSpec", "LOCALAPPDATA", "USERPROFILE", "APPDATA"]
    {
        if let Some(v) = get(k) {
            env.push((k, v));
        }
    }
    let root = get("SystemRoot").map_or_else(|| r"C:\Windows".to_string(), |r| r.to_string_lossy().into_owned());
    env.push((
        "PATH",
        format!(r"{root}\System32;{root};{root}\System32\Wbem;{root}\System32\WindowsPowerShell\v1.0").into(),
    ));
    env
}

fn dry_run_cmd(a: &Args) -> Result<(), String> {
    use std::io::Write;
    let program = &a.words[0];
    let work = Path::new(a.one("--work").unwrap_or(""));
    let root = a.one("--root");
    let setup = |c: &mut Command| {
        if a.clean {
            c.env_clear();
            for (k, v) in clean_env(cfg!(windows), |k| std::env::var_os(k)) {
                c.env(k, v);
            }
        }
    };
    // (the cache the program runs with: its --cache, after the command)
    let cache: Vec<std::ffi::OsString> = a
        .one("--cache")
        .map(|c| vec!["--cache".into(), std::path::absolute(c).unwrap_or_else(|_| c.into()).into_os_string()])
        .unwrap_or_default();
    if a.clean && root.is_none() {
        let start = std::time::Instant::now();
        let mut c = build_core::child::command(program);
        setup(&mut c);
        let s = build_core::child::status(c.args(["sources", "--output", "text"]).args(&cache))
            .map_err(|e| format!("{program} did not start: {e}"))?;
        if !s.success() {
            return Err(format!("{program} sources failed"));
        }
        say(format!("first start: {:.1} s to unpack", start.elapsed().as_secs_f64()));
    }
    let mut outs = Vec::new();
    for up in ["none", "fsr4"] {
        let mut c = build_core::child::command(program);
        setup(&mut c);
        c.arg("compile").arg("--work").arg(work.join(format!("dry-{up}")));
        c.args(["--dry-run", "--upscaler", up, "--output", "text"]).args(&cache);
        if let Some(r) = root {
            c.args(["--root", r]);
        }
        let o = build_core::child::output(&mut c).map_err(|e| format!("{program} did not start: {e}"))?;
        let _ = std::io::stdout().write_all(&o.stdout);
        let _ = std::io::stderr().write_all(&o.stderr);
        if !o.status.success() {
            return Err(format!("the dry run ({up}) failed"));
        }
        outs.push(String::from_utf8_lossy(&o.stdout).into_owned());
    }
    // (the tree it reads: --root, else the checkout it runs in, as the release does; a --clean run without --root
    // reads the tree it carries, whose Rust its own lines show)
    let rust = Path::new(root.unwrap_or(".")).join("longshot-rs").join("Cargo.toml").is_file();
    check_dry_runs(&outs[0], &outs[1], rust)?;
    say("the dry runs passed".into());
    Ok(())
}

// ---- RecompLauncher's release file ----

/// RecompLauncher's release file's name: version, platform.
const RECOMPLAUNCHER: &str = "longshot-builder-{v}-{p}-recomplauncher.zip";
/// Each file's time in the zip: 2026-01-01 00:00, as DOS writes it.
const ZIP_DATE: u16 = ((2026 - 1980) << 9) | (1 << 5) | 1;

/// The version and platform a release's full file's name says (`longshot-builder-<v>-<p>[.exe]`).
fn release_of(name: &str) -> Option<(String, &'static str)> {
    let rest = name.strip_prefix("longshot-builder-")?;
    let rest = rest.strip_suffix(".exe").unwrap_or(rest);
    rd::PLATFORMS.iter().find_map(|(p, _)| {
        let v = rest.strip_suffix(p)?.strip_suffix('-')?;
        (!v.is_empty()).then(|| (v.to_string(), *p))
    })
}

/// A zip of `files` (name, bytes, Unix mode), stored, each file's time [`ZIP_DATE`], made on Unix: the same files give
/// the same bytes (as Python's zipfile wrote it before, byte for byte).
pub fn stored_zip(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, mode) in files {
        let crc = crc32fast::hash(data);
        let at = out.len() as u32;
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let common = |b: &mut Vec<u8>| {
            b.extend(20u16.to_le_bytes()); // (the version needed: 2.0)
            b.extend(flags.to_le_bytes());
            b.extend(0u16.to_le_bytes()); // (stored)
            b.extend(0u16.to_le_bytes()); // (00:00)
            b.extend(ZIP_DATE.to_le_bytes());
            b.extend(crc.to_le_bytes());
            b.extend((data.len() as u32).to_le_bytes());
            b.extend((data.len() as u32).to_le_bytes());
            b.extend((name.len() as u16).to_le_bytes());
            b.extend(0u16.to_le_bytes()); // (no extra field)
        };
        out.extend(0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend(name.as_bytes());
        out.extend(*data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend([20u8, 3]); // (made by 2.0, on Unix)
        common(&mut central);
        central.extend(0u16.to_le_bytes()); // (no comment)
        central.extend(0u16.to_le_bytes()); // (disk 0)
        central.extend(0u16.to_le_bytes()); // (internal attributes)
        central.extend(((0o100000 | mode) << 16).to_le_bytes());
        central.extend(at.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let (at, size) = (out.len() as u32, central.len() as u32);
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend(size.to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

/// The Windows program `exe` made a windowed one: its PE subsystem 2 (Windows GUI), not 3 (console). Started by
/// RecompLauncher, a shortcut or Steam it then opens no console window, and what it starts shares a hidden one
/// (build-core's `hidden_console`); its output still reaches whoever reads its stdout (RecompLauncher's prepare). That
/// one field changes: the image carries no signature, and Windows checks no checksum of a program.
pub(crate) fn windowed(mut exe: Vec<u8>) -> Result<Vec<u8>, String> {
    release::swap::set_subsystem(&mut exe, 2)?;
    Ok(exe)
}

/// RecompLauncher's release file for the full file at `full`, into `out`: the program players start, named as play.rs
/// plays by (`Longshot.exe`, made a windowed program by [`windowed`], or `Longshot.x86_64`), which recomplauncher.json's
/// `start` and `prepare` both run, and `manifest` (recomplauncher.json) beside it, where RecompLauncher reads it.
/// Stored, not compressed (the program is compressed inside already); on Linux the program is the full file byte for
/// byte, on Windows all but its subsystem. Returns its path.
fn recomplauncher(full: &Path, out: Option<&Path>, manifest: &Path) -> Result<PathBuf, String> {
    let name = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (version, platform) = release_of(&name).ok_or_else(|| {
        format!("{} is not named as a release's full file (longshot-builder-<v>-<p>[.exe])", full.display())
    })?;
    let full_abs = std::path::absolute(full).map_err(|e| e.to_string())?;
    let dir = out.map(Path::to_path_buf).unwrap_or_else(|| full_abs.parent().unwrap_or(Path::new(".")).to_path_buf());
    let to = dir.join(RECOMPLAUNCHER.replace("{v}", &version).replace("{p}", platform));
    let program = std::fs::read(full).map_err(|e| format!("{}: {e}", full.display()))?;
    let json = std::fs::read(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let [windows, linux] = crate::play::NAMES;
    let (exe, program) = if name.ends_with(".exe") { (windows, windowed(program)?) } else { (linux, program) };
    let zip = stored_zip(&[(exe, &program, 0o755), ("recomplauncher.json", &json, 0o644)]);
    let mut new = to.as_os_str().to_owned();
    new.push(".new");
    std::fs::write(&new, zip).map_err(|e| format!("{}: {e}", to.display()))?;
    std::fs::rename(&new, &to).map_err(|e| format!("{}: {e}", to.display()))?;
    Ok(to)
}

fn run_parsed(a: &Args) -> Result<(), String> {
    match a.command.as_str() {
        "sources" => sources_cmd(a),
        "unpack" => unpack_cmd(a),
        "attach" => attach_cmd(a),
        "dry-run" => dry_run_cmd(a),
        "last-objects" => {
            use super::release_objects as ro;
            let bytes = |url: &str, most: u64| {
                let dir = Path::new(a.one("--out").unwrap_or("."));
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                let to = dir.join(format!(".download-{}", std::process::id()));
                let got = build_driver::tools::store::download(url, &to, most)
                    .and_then(|()| std::fs::read(&to).map_err(|e| format!("{}: {e}", to.display())));
                let _ = std::fs::remove_file(&to);
                got
            };
            let curl = Net::curl();
            let mirror = mirror();
            let api = a.one("--api").unwrap_or(rd::API);
            // (the full file through FORGEJO_UPLOAD_API's server first, as fetch-bases takes it, then the public address)
            let file = |url: &str, to: &Path, sha: &str, size: u64| {
                if let Some(m) = mirror.as_deref().and_then(|m| rd::through(url, api, m))
                    && (curl.file)(&m, to, sha, size).is_ok()
                {
                    return Ok(());
                }
                (curl.file)(url, to, sha, size)
            };
            let net = ro::Net { json: curl.json, bytes: &bytes, file: &file };
            let l = ro::Last {
                platform: a.one("--platform").unwrap_or(""),
                version: a.one("--version").unwrap_or(""),
                out: Path::new(a.one("--out").unwrap_or(".")),
                api,
                repo: a.one("--repo").unwrap_or(rd::REPO),
            };
            let keys = release::trust::keys(&|k| std::env::var(k).ok());
            if let Some((tag, dir)) = ro::last_objects(&l, &net, &keys, &mut say) {
                say(format!("{tag}'s set: {}", dir.display()));
            }
            Ok(())
        }
        "fetch-bases" => {
            let mirror = mirror();
            let keep = a.one("--keep").map_or(rd::KEEP, |k| k.parse().unwrap_or(rd::KEEP));
            let channel = a.one("--channel").unwrap_or("");
            let b = rd::Bases {
                platform: a.one("--platform").unwrap_or(""),
                channel,
                also: rd::also_of(channel),
                not: a.one("--not").unwrap_or(""),
                out: Path::new(a.one("--out").unwrap_or("")),
                api: a.one("--api").unwrap_or(rd::API),
                mirror: mirror.as_deref(),
                repo: a.one("--repo").unwrap_or(rd::REPO),
                keep,
            };
            rd::fetch_bases(&b, &Net::curl(), &mut say).map(|_| ())
        }
        "deltas" => {
            let d = rd::Deltas {
                full: Path::new(&a.words[0]),
                version: a.one("--version").unwrap_or(""),
                channel: a.one("--channel").unwrap_or(""),
                platform: a.one("--platform").unwrap_or(""),
                bases: a.one("--bases").map(Path::new),
                out: Path::new(a.one("--out").unwrap_or("")),
                stamps: a.one("--stamps").map(Path::new),
            };
            rd::deltas(&d, &mut say).map(|_| ())
        }
        "files" => {
            let mirror = mirror();
            let f = rd::Files {
                full: Path::new(&a.words[0]),
                version: a.one("--version").unwrap_or(""),
                platform: a.one("--platform").unwrap_or(""),
                work: Path::new(a.one("--work").unwrap_or("")),
                runner: a.one("--runner"),
                api: a.one("--api").unwrap_or(rd::API),
                mirror: mirror.as_deref(),
                repo: a.one("--repo").unwrap_or(rd::REPO),
            };
            rd::files(&f, &Net::curl(), &mut say)
        }
        "recomplauncher" => {
            let manifest = checkout(a.one("--root"))?.join("recomplauncher.json");
            let to = recomplauncher(Path::new(&a.words[0]), a.one("--out").map(Path::new), &manifest)?;
            println!("{}", to.display());
            Ok(())
        }
        _ => unreachable!("parse takes no other"),
    }
}

/// Runs the command on its arguments (after `release`).
pub fn run(args: &[String]) -> ExitCode {
    if matches!(args.first().map(String::as_str), Some("--help" | "-h" | "help")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("longshot-builder dev release: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run_parsed(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("longshot-builder dev release: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_dry_run_keeps_what_windows_needs() {
        let get = |k: &str| match k {
            "SystemRoot" => Some(r"C:\Windows".into()),
            "LOCALAPPDATA" => Some(r"C:\Users\p\AppData\Local".into()),
            "USERPROFILE" => Some(r"C:\Users\p".into()),
            "LONGSHOT_LLVM" => Some(r"D:\llvm".into()),
            _ => None,
        };
        let env = clean_env(true, get);
        let keys: Vec<&str> = env.iter().map(|e| e.0).collect();
        assert_eq!(keys, ["SystemRoot", "LOCALAPPDATA", "USERPROFILE", "PATH"]);
        assert!(env[3].1.to_string_lossy().starts_with(r"C:\Windows\System32;"));
        assert_eq!(clean_env(false, get)[0], ("PATH", "/usr/bin:/bin".into()));
    }

    fn args(s: &str) -> Result<Args, String> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    fn scratch(name: &str) -> PathBuf {
        // (a folder of each call's own: tests run side by side, some calling this with one name)
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("dev-release-cmd-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_command_lines_it_takes() {
        let s = args("sources --out p --version v --add a=b --add c=d --tools t").unwrap();
        assert_eq!((s.all("--add").len(), s.one("--tools")), (2, Some("t")));
        assert!(args("sources --out p").is_err(), "--version is needed");
        assert!(args("sources --out p --version v --clean").is_err(), "a flag it does not take");
        assert!(args("dry-run prog --work w --clean").unwrap().clean);
        assert!(args("dry-run --work w").is_err(), "the program is needed");
        assert!(args("fetch-bases --platform mac-arm --channel nightly --out o").is_err());
        assert!(args("fetch-bases --platform linux-x64 --channel beta --out o").is_err());
        assert!(args("fetch-bases --platform linux-x64 --channel nightly --out o --keep x").is_err());
        assert!(args("deltas f --version v --channel stable --platform windows-x64 --out o").is_ok());
        assert!(args("files f --version v --platform linux-x64 --work w --runner wine").is_ok());
        assert!(args("unpack a").is_err());
        assert!(args("recomplauncher f --out o").is_ok());
        assert!(args("release f").is_err());
    }

    #[test]
    fn a_dry_run_is_checked_as_before() {
        let good = "dry run: 812 files to compile besides the lift's, 0 missing\n\
                    dry run: the third-party objects: 40 of 40 in the tools' set\n";
        let fsr = format!("{good}with AMD FSR 4\n");
        assert_eq!(check_dry_runs(good, &fsr, false), Ok(()));
        assert!(check_dry_runs(good, good, false).is_err(), "no AMD FSR 4 where it is asked for");
        assert!(check_dry_runs(&fsr, &fsr, false).is_err(), "AMD FSR 4 where it is not");
        let missing = good.replace(", 0 missing", ", 3 missing");
        assert!(check_dry_runs(&missing, &fsr, false).is_err());
        let none = good.replace("812 files", "0 files");
        assert!(check_dry_runs(&none, &fsr, false).is_err());
        let compiled = good.replace("40 of 40", "39 of 40");
        assert!(check_dry_runs(&compiled, &fsr, false).is_err());
        let zero = good.replace("40 of 40", "0 of 0");
        assert!(check_dry_runs(&zero, &fsr, false).is_err());
        // (a tree with the game's Rust: its prebuilt library, in both runs)
        assert!(check_dry_runs(good, &fsr, true).is_err(), "no prebuilt liblongshot_rs.a");
        let rust = format!("{good}{RUST_PREBUILT}\n");
        let rust_fsr = format!("{rust}with AMD FSR 4\n");
        assert_eq!(check_dry_runs(&rust, &rust_fsr, true), Ok(()));
        assert!(check_dry_runs(&rust, &fsr, true).is_err(), "prebuilt in one run only");
        // (the tree the dry run read had the Rust, though the checkout it ran in did not: still checked)
        assert!(check_dry_runs(&rust, &fsr, false).is_err(), "prebuilt in one run only, Rust unseen in the checkout");
        let cargo =
            format!("{good}dry run: the game's Rust: no prebuilt library of its sources: cargo would build it\n");
        let cargo_fsr = format!("{cargo}with AMD FSR 4\n");
        assert!(check_dry_runs(&cargo, &cargo_fsr, false).is_err(), "cargo would build it");
        assert!(check_dry_runs(&rust, &cargo_fsr, false).is_err(), "cargo would build it in one run");
    }

    #[test]
    fn a_full_file_s_name_says_its_release() {
        assert_eq!(release_of("longshot-builder-v3.2.0-linux-x64"), Some(("v3.2.0".into(), "linux-x64")));
        assert_eq!(
            release_of("longshot-builder-nightly-7-ab-windows-x64.exe"),
            Some(("nightly-7-ab".into(), "windows-x64"))
        );
        assert_eq!(release_of("longshot-builder-linux-x64"), None);
        assert_eq!(release_of("longshot-builder-v1-mac"), None);
    }

    /// The files of a stored zip, read back: (name, bytes, Unix mode).
    fn read_zip(z: &[u8]) -> Vec<(String, Vec<u8>, u32)> {
        let u16_at = |i: usize| u16::from_le_bytes([z[i], z[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes(z[i..i + 4].try_into().unwrap()) as usize;
        let end = z.len() - 22;
        assert_eq!(u32_at(end), 0x0605_4b50);
        let (count, mut at) = (u16_at(end + 10), u32_at(end + 16));
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(at), 0x0201_4b50);
            assert_eq!((z[at + 4], z[at + 5]), (20, 3), "made on Unix");
            let (size, name_len, mode, local) = (u32_at(at + 24), u16_at(at + 28), u32_at(at + 38), u32_at(at + 42));
            let name = String::from_utf8(z[at + 46..at + 46 + name_len].to_vec()).unwrap();
            assert_eq!(u32_at(local), 0x0403_4b50);
            assert_eq!(u16_at(local + 8), 0, "stored");
            let data = local + 30 + u16_at(local + 26) + u16_at(local + 28);
            assert_eq!(crc32fast::hash(&z[data..data + size]) as usize, u32_at(local + 14));
            out.push((name, z[data..data + size].to_vec(), (mode >> 16) as u32));
            at += 46 + name_len;
        }
        out
    }

    #[test]
    fn the_recomplauncher_zip_carries_the_program_and_recomplauncher_json() {
        // (tests/test_recomplauncher_json.py's, which checked the Python this replaces)
        let d = scratch("recomplauncher");
        let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let manifest = checkout.join("recomplauncher.json");
        for (platform, exe) in rd::PLATFORMS {
            let full = d.join(format!("longshot-builder-v3.2.0-{platform}{exe}"));
            let program = if exe.is_empty() { b"the program".to_vec() } else { fake_pe(3) };
            std::fs::write(&full, &program).unwrap();
            let out = recomplauncher(&full, None, &manifest).unwrap();
            assert_eq!(out, d.join(format!("longshot-builder-v3.2.0-{platform}-recomplauncher.zip")));
            let first = std::fs::read(&out).unwrap();
            recomplauncher(&full, None, &manifest).unwrap();
            assert_eq!(std::fs::read(&out).unwrap(), first, "the same files make the same zip");
            let files = read_zip(&first);
            let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
            let name = if exe.is_empty() { "Longshot.x86_64" } else { "Longshot.exe" };
            assert_eq!(names, [name, "recomplauncher.json"], "the program players start, not a builder");
            assert_eq!(files[0].1, if exe.is_empty() { program } else { fake_pe(2) }, "Windows': a windowed program");
            assert_eq!(files[1].1, std::fs::read(&manifest).unwrap());
            assert_eq!((files[0].2 & 0o777, files[1].2 & 0o777), (0o755, 0o644));
        }
        assert!(recomplauncher(&d.join("something-else"), None, &manifest).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A PE image of subsystem `subsystem` (the headers alone).
    fn fake_pe(subsystem: u16) -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        b[0xdc..0xde].copy_from_slice(&subsystem.to_le_bytes());
        b
    }

    #[test]
    fn the_windows_program_is_made_a_windowed_one() {
        assert_eq!(windowed(fake_pe(3)).unwrap(), fake_pe(2));
        assert_eq!(windowed(fake_pe(2)).unwrap(), fake_pe(2));
        assert!(windowed(fake_pe(1)).is_err(), "a driver's");
        assert!(windowed(b"the program".to_vec()).is_err());
        let mut no_header = fake_pe(3);
        no_header[0x98] = 0;
        assert!(windowed(no_header).is_err());
    }

    #[test]
    fn the_zip_is_python_s_byte_for_byte() {
        // (zipfile.ZipFile(..., ZIP_STORED) with ZipInfo("a", (2026, 1, 1, 0, 0, 0)), create_system 3, mode 0o755,
        // writestr(b"hi"): the bytes the release's zips had before)
        let want = "504b03041400000000000000215cac2a93d8020000000200000001000000616869504b010214031400000000000000215cac2a93d8\
                    0200000002000000010000000000000000000000ed810000000061504b050600000000010001002f000000210000000000";
        let want: String = want.chars().filter(|c| !c.is_whitespace()).collect();
        let got: String = stored_zip(&[("a", b"hi", 0o755)]).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, want);
    }
}

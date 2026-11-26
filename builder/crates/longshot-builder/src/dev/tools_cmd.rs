//! `longshot-builder dev tools`: tools.lock, the build tools longshot-builder carries (docs/dev/reference/releases.md,
//! "tools.lock" and "Bumping a tool"), over build_driver::tools (`lock`, `archive`, `store`).
//!
//! ```text
//! check [--require [<name> ...]]   the lock read and checked, and a lock without a tool a builder needs refused;
//!                                  --require: also each line's copy in Longshot's store with its size and SHA-256 (the
//!                                  release, which uploads none: the Make tools workflow does); --require <name>...: also
//!                                  those tools named
//! install <folder> [--system <s>] [--cache <folder>] [--only <name> ...]
//!                                  each tool of the system unpacked at its dest under folder, its stamp in
//!                                  folder/.tools/<name>: a release's bundle, or a checkout (its tools/, which
//!                                  `--root <checkout>` takes)
//! add <name> <version> <archive> [--dest <folder>] [--prefix <folder>] [--system <s>] [--no-upload]
//!                                  an archive uploaded to the store and written into the lock in place of the tool's
//!                                  line (dest: tools/<name>); --no-upload: the line only (builder/tools/make.sh's add.sh)
//! make <folder> --out <archive.tar.zst>
//!                                  a folder as a tool's archive, the same files giving the same bytes (builder/tools/
//!                                  make.sh packs each recipe's with it)
//! ```
//!
//! `--lock <file>` before the command: another lock than the checkout's (the first tools.lock at or above the folder it
//! runs in). `--system` is `windows-x64` unless given. Exit status 1 when something is wrong, 2 for a command line it
//! does not take.

use build_driver::tools::archive;
use build_driver::tools::lock::{self, SYSTEMS, Tool};
use build_driver::tools::store::{self, InStore};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: longshot-builder dev tools [--lock <file>] check [--require [<name> ...]]
       longshot-builder dev tools [--lock <file>] install <folder> [--system <s>] [--cache <folder>] [--only <name> ...]
       longshot-builder dev tools [--lock <file>] add <name> <version> <archive> [--dest <folder>] [--prefix <folder>]
                                                  [--system <s>] [--no-upload]
       longshot-builder dev tools make <folder> --out <archive.tar.zst>";

/// The command line after `tools`: words, options with one value, options with several (until the next option), flags.
#[derive(Debug, Default)]
struct Args {
    words: Vec<String>,
    lock: Option<PathBuf>,
    system: Option<String>,
    cache: Option<PathBuf>,
    dest: Option<String>,
    prefix: Option<String>,
    out: Option<PathBuf>,
    require: Option<Vec<String>>,
    only: Option<Vec<String>>,
    no_upload: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        let mut value = || -> Result<String, String> {
            let v = args.get(i).cloned().ok_or_else(|| format!("{arg} needs a value"))?;
            i += 1;
            Ok(v)
        };
        match arg {
            "--lock" => a.lock = Some(value()?.into()),
            "--system" => {
                let s = value()?;
                if !SYSTEMS.contains(&s.as_str()) {
                    return Err(format!("--system is one of {}", SYSTEMS.join(", ")));
                }
                a.system = Some(s);
            }
            "--cache" => a.cache = Some(value()?.into()),
            "--dest" => a.dest = Some(value()?),
            "--prefix" => a.prefix = Some(value()?),
            "--out" => a.out = Some(value()?.into()),
            "--no-upload" => a.no_upload = true,
            "--require" | "--only" => {
                let mut names = Vec::new();
                while i < args.len() && !args[i].starts_with("--") {
                    names.push(args[i].clone());
                    i += 1;
                }
                *(if arg == "--require" { &mut a.require } else { &mut a.only }) = Some(names);
            }
            o if o.starts_with("--") => return Err(format!("unknown argument {o:?}")),
            w => a.words.push(w.to_string()),
        }
    }
    let n = |want: usize| a.words.len() == want + 1;
    let fits = match a.words.first().map(String::as_str) {
        Some("check") => n(0),
        Some("install") => n(1),
        Some("add") => n(3),
        Some("make") => n(1) && a.out.is_some(),
        _ => false,
    };
    if fits { Ok(a) } else { Err("a command and its arguments are needed".into()) }
}

/// The checkout's lock: the first tools.lock at or above the current folder.
fn default_lock() -> Result<PathBuf, String> {
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    here.ancestors()
        .map(|d| d.join("tools.lock"))
        .find(|p| p.is_file())
        .ok_or_else(|| "no tools.lock at or above this folder: name one with --lock".into())
}

fn check(tools: &[Tool], require: Option<&[String]>) -> bool {
    let missing: Vec<String> = match require {
        None | Some([]) => lock::missing_required(tools),
        Some(names) => names.iter().filter(|n| !tools.iter().any(|t| &t.name == *n)).cloned().collect(),
    };
    for t in tools {
        println!("{:<12} {:<36} {} {:>6} MB  {}", t.name, t.version, t.system, t.size >> 20, t.urls[0]);
    }
    if !missing.is_empty() {
        eprintln!(
            "error: tools.lock is missing: {} (a builder needs them); docs/dev/reference/releases.md, Bumping a tool",
            missing.join(", ")
        );
        return false;
    }
    if require.is_none() {
        return true;
    }
    let gone: Vec<String> = tools
        .iter()
        .filter_map(|t| match store::exists(t) {
            InStore::There { size: Some(size), sha256: Some(sha) } if size == t.size && sha == t.sha256 => None,
            InStore::There { size, sha256 } => {
                Some(format!("{} ({}): {size:?} bytes, SHA-256 {sha256:?}", t.name, t.system))
            }
            InStore::Missing => Some(format!("{} ({}): not there", t.name, t.system)),
            InStore::Unreached(why) => Some(format!("{} ({}): the store could not be asked ({why})", t.name, t.system)),
        })
        .collect();
    if !gone.is_empty() {
        eprintln!(
            "error: not in Longshot's store as tools.lock names them: {}; the Make tools workflow (.forgejo/workflows/\
             tools.yml) uploads them, docs/dev/reference/releases.md, Bumping a tool",
            gone.join("; ")
        );
        return false;
    }
    true
}

fn run_parsed(a: &Args) -> Result<bool, String> {
    let cmd = a.words[0].as_str();
    if cmd == "make" {
        archive::pack(Path::new(&a.words[1]), a.out.as_deref().unwrap_or(Path::new("")))?;
        return Ok(true);
    }
    let lock_path = match &a.lock {
        Some(p) => p.clone(),
        None => default_lock()?,
    };
    let tools = lock::load(&lock_path)?;
    let system = a.system.as_deref().unwrap_or("windows-x64");
    match cmd {
        "check" => return Ok(check(&tools, a.require.as_deref())),
        "install" => {
            let chosen: Vec<&Tool> = tools
                .iter()
                .filter(|t| t.system == system && a.only.as_ref().is_none_or(|o| o.contains(&t.name)))
                .collect();
            let cache = a
                .cache
                .clone()
                .unwrap_or_else(|| lock_path.parent().unwrap_or(Path::new(".")).join("build-tools-cache"));
            archive::install(Path::new(&a.words[1]), &chosen, &cache, &store::download)?;
        }
        "add" => {
            let (name, version, archive_path) = (&a.words[1], &a.words[2], Path::new(&a.words[3]));
            let (sha256, size) = archive::sha256_file(archive_path)?;
            let mut t = Tool {
                name: name.clone(),
                version: version.clone(),
                system: system.to_string(),
                sha256,
                size,
                dest: a.dest.clone().unwrap_or_else(|| format!("tools/{name}")),
                prefix: a.prefix.clone().unwrap_or_else(|| "-".into()),
                urls: Vec::new(),
            };
            t.urls = t.store_urls();
            if !a.no_upload {
                store::put(archive_path, &t)?;
            }
            lock::write_line(&t, &lock_path)?;
            println!("tools.lock: {name} {version} {system} {}", t.sha256);
        }
        _ => unreachable!("parse takes no other"),
    }
    Ok(true)
}

/// Runs the command on its arguments (after `tools`).
pub fn run(args: &[String]) -> ExitCode {
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("longshot-builder dev tools: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run_parsed(&a) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("longshot-builder dev tools: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<Args, String> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn the_command_lines_it_takes() {
        assert_eq!(args("check").unwrap().require, None);
        assert_eq!(args("check --require").unwrap().require, Some(vec![]));
        assert_eq!(args("check --require llvm cpm").unwrap().require, Some(vec!["llvm".into(), "cpm".into()]));
        let i = args("install . --system linux-x64 --only cpm --cache c").unwrap();
        assert_eq!(
            (i.only, i.system.as_deref(), i.cache),
            (Some(vec!["cpm".into()]), Some("linux-x64"), Some("c".into()))
        );
        assert!(args("install . --system mac").is_err());
        assert!(args("install").is_err());
        assert!(args("make dir").is_err(), "--out is needed");
        assert!(args("missing").is_err(), "a release uploads no tool: the Make tools workflow does");
        assert!(args("publish sdl a.tar.zst --system windows-x64").is_err());
        assert!(args("add n 1 a.tar.zst --no-upload").unwrap().no_upload);
        assert!(args("repack x").is_err());
    }

    #[test]
    fn check_requires_every_tool() {
        let mut text = String::new();
        for (s, names) in lock::REQUIRED {
            for n in names.iter().filter(|n| !(s == "linux-x64" && **n == "sysroot")) {
                let mut t = Tool {
                    name: n.to_string(),
                    version: "1".into(),
                    system: s.into(),
                    sha256: "a".repeat(64),
                    size: 10,
                    dest: format!("tools/{n}"),
                    prefix: "-".into(),
                    urls: vec![],
                };
                t.urls = t.store_urls();
                text += &(t.line() + "\n");
            }
        }
        assert!(!check(&lock::parse(&text).unwrap(), None), "sysroot (linux-x64) is missing");
        assert!(!check(&lock::parse(&text).unwrap(), Some(&["nothing".into()])), "a tool named that is not there");
    }
}

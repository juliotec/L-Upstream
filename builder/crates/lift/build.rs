//! The lifter's own key (`LIFTER_SOURCES`): a hash of the sources of every crate that shapes the lifted code (the
//! decoder, the discovery, the semantics, the mid IR, the passes, the IR writer, the profile, this crate, the edits' lowering) and the runtime written
//! beside it, so a lift's cache is new exactly when the lifter is; and `EDITS_SOURCES`, a hash of the edits and sites crates (the
//! patches, the sites' placement and code), which the whole lift's key holds. Tests are left out: they change no output.
//! The lifter's key also holds the compiler that builds it (`rustc -V`) and the Cargo.lock entries of the crates.io
//! crates it is built with (this crate's dependencies, all the way down): either can change the lifted code too.

use std::path::{Path, PathBuf};

include!("key_crates.rs");

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "tests" {
                files(&p, out);
            }
        } else if name != "tests.rs" && (name.ends_with(".rs") || name.ends_with(".c") || name.ends_with(".h")) {
            out.push(p);
        }
    }
}

/// FNV-1a, 64 bits, from `seed`.
fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The hash of these files (named by their paths from `crates`), each a rerun trigger.
fn hash(crates: &Path, all: Vec<PathBuf>) -> String {
    let mut rel: Vec<(String, PathBuf)> = all
        .into_iter()
        .map(|p| (p.strip_prefix(crates).unwrap_or(&p).to_string_lossy().replace('\\', "/"), p))
        .collect();
    rel.sort();
    let (mut a, mut b) = (0xcbf2_9ce4_8422_2325u64, 0x8422_2325_cbf2_9ce4u64);
    for (name, path) in &rel {
        println!("cargo:rerun-if-changed={}", path.display());
        let text = std::fs::read(path).expect("a source file");
        for part in [name.as_bytes(), &(text.len() as u64).to_le_bytes(), &text] {
            a = fnv(a, part);
            b = fnv(b, part);
        }
    }
    format!("{a:016x}{b:016x}")
}

/// The Cargo.lock entries of the registry crates `root` is built with, its dependencies' dependencies too, in the
/// lock's order (the workspace's own crates, which have no `source`, are in the key by their sources)
fn locked_dependencies(lock: &str, root: &str) -> String {
    struct Package<'a> {
        name: &'a str,
        version: &'a str,
        text: &'a str,
        dependencies: Vec<&'a str>,
        registry: bool,
    }
    let quoted = |line: &str, key: &str| -> Option<String> {
        let rest = line.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim();
        Some(rest.trim_matches('"').to_string())
    };
    let mut packages = Vec::new();
    for text in lock.split("[[package]]").skip(1) {
        let (mut name, mut version) = ("", "");
        let (mut dependencies, mut in_deps, mut registry) = (Vec::new(), false, false);
        for line in text.lines() {
            let t = line.trim();
            if in_deps {
                if t.starts_with(']') {
                    in_deps = false;
                } else {
                    dependencies.push(t.trim_end_matches(',').trim_matches('"'));
                }
            } else if t.starts_with("dependencies") {
                in_deps = !t.ends_with(']');
            } else if quoted(t, "name").is_some() {
                name = t.split('"').nth(1).unwrap_or("");
            } else if quoted(t, "version").is_some() {
                version = t.split('"').nth(1).unwrap_or("");
            } else if t.starts_with("source") {
                registry = true;
            }
        }
        packages.push(Package { name, version, text, dependencies, registry });
    }
    // (a dependency is "name" or, where the lock has two versions of it, "name version")
    let find = |dep: &str| -> Option<usize> {
        let mut it = dep.split(' ');
        let (name, version) = (it.next()?, it.next());
        packages.iter().position(|p| p.name == name && version.is_none_or(|v| p.version == v))
    };
    let mut reached = vec![false; packages.len()];
    let mut todo: Vec<usize> = find(root).into_iter().collect();
    while let Some(i) = todo.pop() {
        if std::mem::replace(&mut reached[i], true) {
            continue;
        }
        todo.extend(packages[i].dependencies.iter().filter_map(|d| find(d)));
    }
    packages.iter().zip(&reached).filter(|(p, r)| **r && p.registry).map(|(p, _)| p.text).collect()
}

fn main() {
    println!("cargo:rerun-if-changed=key_crates.rs");
    let here = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let crates = here.parent().expect("crates/").to_path_buf();
    let mut dirs: Vec<PathBuf> = CRATES.iter().map(|c| crates.join(c).join("src")).collect();
    dirs.push(crates.parent().expect("builder/").join("rt"));
    let mut all: Vec<PathBuf> = FILES.iter().map(|f| crates.join(f)).collect();
    for d in &dirs {
        println!("cargo:rerun-if-changed={}", d.display());
        files(d, &mut all);
    }
    let sources = hash(&crates, all);
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(&rustc)
        .arg("-V")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    // (the workspace's one Cargo.lock, at the repository's root: builder/crates/lift/../../..)
    let lock_path = crates.parent().and_then(Path::parent).expect("the repository").join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("{}: {e}: the lifter's key holds its dependencies' versions", lock_path.display()));
    if !lock.split("[[package]]").any(|p| p.lines().any(|l| l.trim() == "name = \"lift\"")) {
        panic!("{} has no lift package: not the workspace's lock", lock_path.display());
    }
    let deps = locked_dependencies(&lock, "lift");
    let (mut a, mut b) = (0xcbf2_9ce4_8422_2325u64, 0x8422_2325_cbf2_9ce4u64);
    for part in [sources.as_bytes(), version.as_bytes(), deps.as_bytes()] {
        a = fnv(a, &(part.len() as u64).to_le_bytes());
        a = fnv(a, part);
        b = fnv(b, &(part.len() as u64).to_le_bytes());
        b = fnv(b, part);
    }
    println!("cargo:rustc-env=LIFTER_SOURCES={a:016x}{b:016x}");
    // (the edits' data, compiled in: the patches, each site form's code. A change of them is a lift to do, not a new
    // lifter: the functions' own keys then say which units it changes)
    let mut edits = Vec::new();
    for c in ["edits", "sites"] {
        let dir = crates.join(c).join("src");
        println!("cargo:rerun-if-changed={}", dir.display());
        files(&dir, &mut edits);
    }
    println!("cargo:rustc-env=EDITS_SOURCES={}", hash(&crates, edits));
}

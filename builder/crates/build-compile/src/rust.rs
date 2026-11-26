//! The Rust the game links: `liblongshot_rs.a`, the workspace's one staticlib (longshot-rs/, over game/crates/).
//!
//! A player's build compiles no Rust: the release builds the library for each platform and carries it in the tree,
//! `longshot-rs/prebuilt/<platform>/` with the key of the sources it was built from beside it ([`KEY_FILE`]). A tree
//! whose key is that one takes it as it is. Any other tree (a developer's, the sources changed) builds it with cargo:
//! `cargo build -p longshot-rs --profile game --target <the game's>`, into the work folder, so a player's build time
//! does not grow and a developer's change is what the game links.
//!
//! The key ([`source_key`]) is the Rust's sources and nothing else: the workspace's `Cargo.toml` and `Cargo.lock`, and
//! every file of `longshot-rs/` and `game/crates/` but the prebuilt libraries. `longshot-builder dev rust-key` prints
//! it, for the release to write beside the library it built.

use build_core::key::Key;
use build_core::report::Reporter;
use build_core::{Error, IoContext, Result};
use std::path::{Path, PathBuf};

/// The file beside a prebuilt library that holds the key of the sources it was built from.
pub const KEY_FILE: &str = "sources.key";
/// The library's file name (llvm-mingw's and Linux's: `lib<name>.a`).
pub const LIBRARY: &str = "liblongshot_rs.a";

/// The Rust the game is linked with, for one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Linux, x86-64 (glibc).
    Linux,
    /// Windows through llvm-mingw, the toolchain the host's C uses.
    WindowsGnu,
    /// Windows through MSVC's link (a developer's build without llvm-mingw).
    WindowsMsvc,
}

impl Target {
    /// The target of a game built with `tc`: llvm-mingw's Windows, MSVC's, or Linux.
    pub fn of(tc: &crate::toolchain::Toolchain) -> Target {
        match (tc.windows, tc.mingw.is_some()) {
            (false, _) => Target::Linux,
            (true, true) => Target::WindowsGnu,
            (true, false) => Target::WindowsMsvc,
        }
    }

    /// Rust's name of the target.
    pub fn triple(self) -> &'static str {
        match self {
            Target::Linux => "x86_64-unknown-linux-gnu",
            Target::WindowsGnu => "x86_64-pc-windows-gnullvm",
            Target::WindowsMsvc => "x86_64-pc-windows-msvc",
        }
    }

    /// The prebuilt library's folder under `longshot-rs/prebuilt` (the release's platform names).
    pub fn platform(self) -> &'static str {
        match self {
            Target::Linux => "linux-x64",
            Target::WindowsGnu => "windows-x64",
            Target::WindowsMsvc => "windows-x64-msvc",
        }
    }

    /// The library's file name.
    pub fn library(self) -> &'static str {
        match self {
            Target::WindowsMsvc => "longshot_rs.lib",
            _ => LIBRARY,
        }
    }

    /// What Rust's standard library needs of the system, past what the host already links.
    pub fn system_libs(self) -> &'static [&'static str] {
        match self {
            Target::Linux => &[],
            Target::WindowsGnu | Target::WindowsMsvc => &["-lntdll", "-luserenv", "-lws2_32", "-lbcrypt"],
        }
    }
}

/// Whether the tree has Rust for the game (a tree from before longshot-rs has none, and links none).
pub fn has_rust(root: &Path) -> bool {
    root.join("longshot-rs").join("Cargo.toml").is_file()
}

/// The files the key reads, by their path from the tree's root (sorted).
fn sources(root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for f in ["Cargo.toml", "Cargo.lock"] {
        if root.join(f).is_file() {
            out.push(f.to_string());
        }
    }
    let mut todo = vec![PathBuf::from("longshot-rs"), PathBuf::from("game").join("crates")];
    while let Some(rel) = todo.pop() {
        let dir = root.join(&rel);
        if !dir.is_dir() {
            continue;
        }
        for e in std::fs::read_dir(&dir).at(&dir)? {
            let e = e.at(&dir)?;
            let name = e.file_name().to_string_lossy().into_owned();
            let path = rel.join(&name);
            if e.file_type().at(e.path())?.is_dir() {
                // (the prebuilt libraries are made from the key, never part of it; a target folder is cargo's)
                if !(rel == Path::new("longshot-rs") && name == "prebuilt") && name != "target" {
                    todo.push(path);
                }
            } else {
                out.push(path.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The key of the tree's Rust: its files' paths and bytes (line endings as they are).
pub fn source_key(root: &Path) -> Result<String> {
    let mut k = Key::new();
    k.text("format", "longshot-rs 1");
    for rel in sources(root)? {
        let path = root.join(&rel);
        k.part(&rel, &std::fs::read(&path).at(&path)?);
    }
    Ok(k.hex())
}

/// The prebuilt library of `target` in the tree when it was built from these sources: its path, else `None`.
pub fn prebuilt(root: &Path, target: Target, key: &str) -> Option<PathBuf> {
    let dir = root.join("longshot-rs").join("prebuilt").join(target.platform());
    let lib = dir.join(target.library());
    let held = std::fs::read_to_string(dir.join(KEY_FILE)).ok()?;
    (held.trim() == key && lib.is_file()).then_some(lib)
}

/// The library to link for `target`: the prebuilt one when the tree's sources are its, else cargo's build of them into
/// `work` (`cargo`: `LONGSHOT_CARGO`'s, else the PATH's). `None` when the tree has no Rust.
pub fn library(
    root: &Path,
    work: &Path,
    target: Target,
    cargo: Option<&Path>,
    r: &Reporter,
) -> Result<Option<PathBuf>> {
    if !has_rust(root) {
        return Ok(None);
    }
    let key = source_key(root)?;
    if let Some(lib) = prebuilt(root, target, &key) {
        r.log(format!("the game's Rust: the prebuilt {} ({})", target.platform(), &key[..12]));
        return Ok(Some(lib));
    }
    let cargo = cargo.map(Path::to_path_buf).or_else(|| crate::toolchain::which("cargo"));
    // (the root Cargo.toml's workspace holds the builder's crates too: a player's tree, which carries no builder/,
    // cannot be built by cargo, with or without it)
    let (Some(cargo), true) = (cargo, root.join("builder").is_dir()) else {
        return Err(Error::player(
            "rust_missing",
            format!(
                "The game's Rust (longshot-rs) has no prebuilt library for these sources ({}), and nothing here to build \
                 it with: in a checkout, install Rust (mise install) or set LONGSHOT_CARGO; a release's tree carries the \
                 library, so take its sources again.",
                &key[..12]
            ),
        ));
    };
    let target_dir = work.join("rust-target");
    let t = std::time::Instant::now();
    let out = std::process::Command::new(&cargo)
        .args(["build", "--locked", "--profile", "game", "-p", "longshot-rs", "--target", target.triple()])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .at(&cargo)?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        let mut tail = said.lines().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        // (the target's standard library missing: rustup's toolchain has only its host's; mise.toml's lists the game's)
        if said.contains("can't find crate for `core`") || said.contains("can't find crate for `std`") {
            tail.push_str(&format!(
                "\n(the Rust target {} is not installed: mise install, or rustup target add {})",
                target.triple(),
                target.triple()
            ));
        }
        return Err(Error::tool("cargo build -p longshot-rs", tail));
    }
    let lib = target_dir.join(target.triple()).join("game").join(target.library());
    if !lib.is_file() {
        return Err(Error::tool("cargo build -p longshot-rs", format!("no {} after the build", lib.display())));
    }
    r.log(format!("the game's Rust: built with cargo for {} ({:.1} s)", target.triple(), t.elapsed().as_secs_f64()));
    Ok(Some(lib))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let d = crate::testdir(name);
        std::fs::create_dir_all(d.join("longshot-rs/src")).unwrap();
        std::fs::create_dir_all(d.join("game/crates/oot/src")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(d.join("longshot-rs/Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(d.join("game/crates/oot/src/fov.rs"), "fn fov() {}\n").unwrap();
        d
    }

    #[test]
    fn the_key_is_the_rust_sources_and_nothing_else() {
        let d = tree("rust-key");
        let a = source_key(&d).unwrap();
        std::fs::create_dir_all(d.join("longshot-rs/prebuilt/linux-x64")).unwrap();
        std::fs::write(d.join("longshot-rs/prebuilt/linux-x64/liblongshot_rs.a"), "lib").unwrap();
        std::fs::create_dir_all(d.join("host")).unwrap();
        std::fs::write(d.join("host/x.c"), "int x;").unwrap();
        assert_eq!(source_key(&d).unwrap(), a, "a prebuilt library and the host's C are not the Rust's sources");
        std::fs::write(d.join("game/crates/oot/src/fov.rs"), "fn fov() { }\n").unwrap();
        assert_ne!(source_key(&d).unwrap(), a, "a source changed");
    }

    #[test]
    fn the_prebuilt_library_is_taken_only_for_its_own_sources() {
        let d = tree("rust-prebuilt");
        let key = source_key(&d).unwrap();
        let dir = d.join("longshot-rs/prebuilt/linux-x64");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LIBRARY), "lib").unwrap();
        std::fs::write(dir.join(KEY_FILE), "0123\n").unwrap();
        assert_eq!(prebuilt(&d, Target::Linux, &key), None, "another key");
        std::fs::write(dir.join(KEY_FILE), format!("{key}\n")).unwrap();
        assert_eq!(prebuilt(&d, Target::Linux, &key), Some(dir.join(LIBRARY)));
        assert_eq!(prebuilt(&d, Target::WindowsGnu, &key), None, "another platform's folder");
        assert!(!has_rust(&d.join("host")));
    }
}

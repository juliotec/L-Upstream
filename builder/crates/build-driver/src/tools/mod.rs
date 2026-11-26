//! The build tools a build compiles with: tools.lock's compiler, sysroot and libraries, which the builder carries in its
//! bundle, each unpacked at its line's dest (`<root>/tools/<name>`: `longshot-builder dev tools install`, the release's
//! bundle), and the stamps a launcher keys shader caches by (`longshot-builder stamps`). The lock itself ([`lock`]), the
//! tools' archives ([`archive`]) and Longshot's store ([`store`]): `longshot-builder dev tools`.
//!
//! The compile reads them as the variables of the table below ([`TOOLS`]), set to the bundle's folders: a player's PC's
//! own `RX_CPM`, `LONGSHOT_MINGW`, ... never stand in for the builder's tools. A developer chooses their own with
//! `LONGSHOT_OWN_TOOLS=1` (the variables, `EMSDK` and the PATH, as before: docs/dev/reference/build.md, "The build
//! tools"); a checkout without them installed (`longshot-builder dev tools install <checkout> --system <s>`) takes the
//! variables too.

pub mod archive;
pub mod lock;
pub mod store;

use crate::env;
use build_core::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Each tool's system, name, and the variable that names its folder to the compile (build-compile's options.rs): the
/// one table of them.
pub const TOOLS: [(&str, &str, &str); 8] = [
    ("linux-x64", "llvm", "LONGSHOT_LLVM"),
    ("linux-x64", "sysroot", "LONGSHOT_SYSROOT"),
    ("linux-x64", "ffmpeg", "LONGSHOT_FFMPEG"),
    ("linux-x64", "sdl2", "LONGSHOT_SDL2"),
    ("linux-x64", "cpm", "RX_CPM"),
    ("windows-x64", "llvm-mingw", "LONGSHOT_MINGW"),
    ("windows-x64", "cpm", "RX_CPM"),
    ("windows-x64", "sdl", "LONGSHOT_SDL_DIR"),
];

/// The tools whose archives the compiler stamp is of: what compiles the renderer and its shader translator.
pub fn compiler_tools(system: &str) -> &'static [&'static str] {
    if system == "windows-x64" { &["llvm-mingw", "cpm"] } else { &["llvm", "sysroot", "cpm"] }
}

/// The system a build makes the game for: Windows on Windows or with `RX_TARGET=windows`, else Linux.
pub fn target_system(get: impl Fn(&str) -> Option<String>) -> &'static str {
    if cfg!(windows) || get("RX_TARGET").is_some_and(|t| t.eq_ignore_ascii_case("windows")) {
        "windows-x64"
    } else {
        "linux-x64"
    }
}

/// The builder's tools, by the variable that names each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolVars {
    /// The bundle's folders.
    pub folders: BTreeMap<String, PathBuf>,
    /// `LONGSHOT_OWN_TOOLS=1`: the environment's variables alone.
    pub own: bool,
}

impl ToolVars {
    /// The tools the tree at `root` carries for the build's system, at `<root>/tools/<name>`. A compiler for another
    /// PC (llvm-mingw's Windows programs, for a Windows game built on Linux) is not taken.
    pub fn of_tree(root: &Path) -> ToolVars {
        let own = build_compile::options::on(env("LONGSHOT_OWN_TOOLS").as_deref());
        let system = target_system(env);
        let mut folders = BTreeMap::new();
        for (sys, name, var) in TOOLS {
            let here = !(name == "llvm-mingw" && !cfg!(windows));
            let dir = root.join("tools").join(name);
            if sys == system && here && dir.is_dir() {
                folders.insert(var.to_string(), dir);
            }
        }
        ToolVars { folders, own }
    }

    /// The value of variable `name` as the compile sees it: the bundle's tool, else (a developer's) the environment's.
    pub fn var(&self, name: &str) -> Option<String> {
        let tool = if self.own { None } else { self.folders.get(name) };
        tool.map(|p| p.display().to_string()).or_else(|| env(name))
    }

    /// The compile's switches: the environment's, the tools' folders over their variables.
    pub fn settings(&self) -> build_compile::Settings {
        build_compile::Settings::from_lookup(|n| self.var(n))
    }

    /// What the build log says of them.
    pub fn describe(&self) -> String {
        if self.own {
            return "the build tools: the developer's own (LONGSHOT_OWN_TOOLS)".into();
        }
        if self.folders.is_empty() {
            return "the build tools: none in the tree (tools/), the environment's variables and the PATH".into();
        }
        let names: Vec<String> =
            self.folders.values().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
        format!("the build tools: the builder's own ({})", names.join(", "))
    }
}

/// The compiler stamp: the SHA-256 of `<name> <sha256>\n` for each of [`compiler_tools`] in that order (two builders
/// with the same one compile the renderer and its shaders alike); `None` when the lock lacks one of them.
pub fn compiler_stamp(tools: &[lock::Tool], system: &str) -> Option<String> {
    use sha2::Digest;
    let mut text = String::new();
    for n in compiler_tools(system) {
        text += &format!("{n} {}\n", lock::named(tools, n, system).ok()?.sha256);
    }
    Some(format!("{:x}", sha2::Sha256::digest(text.as_bytes())))
}

/// The two stamps of the tree at `root` (`longshot-builder stamps`): the compiler's ([`compiler_stamp`], of
/// `<root>/tools.lock`) and the shader translator's (the renderer's `LONGSHOT_SHADER_STAMP`, of `gpu/vk`'s sources).
pub fn stamps(root: &Path) -> Result<(String, String)> {
    let path = root.join("tools.lock");
    let lock = lock::load(&path).map_err(|e| Error::player("tools_incomplete", e))?;
    let system = target_system(env);
    let compiler = compiler_stamp(&lock, system).ok_or_else(|| {
        Error::player("tools_incomplete", format!("{} names no compiler for {system}.", path.display()))
    })?;
    let translator = build_compile::generated::shader_stamp::stamp(&root.join("gpu").join("vk"))?;
    Ok((compiler, translator))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_of(c: char) -> Vec<lock::Tool> {
        let l = |n: &str, s: &str, x: char| lock::tests::line(n, "1", s, &x.to_string().repeat(64), 5);
        let text = [
            "# a comment".to_string(),
            l("llvm", "linux-x64", 'a'),
            l("sysroot", "linux-x64", 'b'),
            l("cpm", "linux-x64", c),
            l("cpm", "windows-x64", 'd'),
        ];
        lock::parse(&text.join("\n")).unwrap()
    }

    #[test]
    fn the_compiler_stamp_is_of_the_compiler_s_lines() {
        use sha2::Digest;
        let h = |c: char| c.to_string().repeat(64);
        let want =
            format!("{:x}", sha2::Sha256::digest(format!("llvm {}\nsysroot {}\ncpm {}\n", h('a'), h('b'), h('c'))));
        assert_eq!(compiler_stamp(&lock_of('c'), "linux-x64"), Some(want));
        assert_eq!(compiler_stamp(&lock_of('c'), "windows-x64"), None, "no llvm-mingw line");
        assert_ne!(compiler_stamp(&lock_of('e'), "linux-x64"), compiler_stamp(&lock_of('c'), "linux-x64"));
    }

    #[test]
    fn the_bundle_s_tools_win_over_the_pc_s_variables() {
        let v = ToolVars { folders: [("RX_CPM".to_string(), PathBuf::from("/b/tools/cpm"))].into(), own: false };
        assert_eq!(v.var("RX_CPM").as_deref(), Some("/b/tools/cpm"));
        assert_eq!(v.settings().cpm, Some(PathBuf::from("/b/tools/cpm")));
        let own = ToolVars { own: true, ..v };
        assert_ne!(own.var("RX_CPM").as_deref(), Some("/b/tools/cpm"));
    }

    #[test]
    fn a_tree_s_tools_are_found_at_their_dests() {
        let d = crate::scratch("tools");
        for n in ["llvm", "sysroot", "cpm", "sdl"] {
            std::fs::create_dir_all(d.join("tools").join(n)).unwrap();
        }
        let v = ToolVars::of_tree(&d);
        if target_system(env) == "linux-x64" {
            assert_eq!(v.folders.get("LONGSHOT_LLVM"), Some(&d.join("tools/llvm")));
            assert_eq!(v.folders.get("RX_CPM"), Some(&d.join("tools/cpm")));
            assert!(!v.folders.contains_key("LONGSHOT_SDL_DIR"), "a Windows tool for a Linux build");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}

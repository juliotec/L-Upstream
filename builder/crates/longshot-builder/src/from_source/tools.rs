//! The build tools a source's tools.lock names for this system, each placed at its dest in the source's tree (where
//! its builder's compile takes them: build_driver::tools::ToolVars), with the code `dev tools install` uses
//! (build_driver::tools::archive: each archive checked by its size and SHA-256, unpacked safely). A tool the running
//! builder carries with the same SHA-256 (its own tools.lock's line) is linked from what it unpacked already: nothing
//! downloaded. Any other is downloaded once into `<cache>/from-source/tools/<name>-<sha256>/` and kept by that hash,
//! the archive dropped once unpacked. These are the source's pins: a pull request's tools.lock is the pull request's.

use build_core::{Error, Result};
use build_driver::tools::archive;
use build_driver::tools::lock::{self, Tool};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// This system's name in tools.lock.
pub fn system() -> &'static str {
    if cfg!(windows) { "windows-x64" } else { "linux-x64" }
}

fn failed(text: impl Into<String>) -> Error {
    Error::player("tools_download_failed", text)
}

/// The tools the running builder carries that a source may take as they are: by name, the folder it unpacked and the
/// SHA-256 its own tools.lock gives that tool's archive for this system.
pub type Carried = BTreeMap<String, (PathBuf, String)>;

/// What the running builder carries (its pack, unpacked into `cache` if it is not yet): its tools, and its tree's
/// folder (RenHDrum's library is in it). Nothing for a developer's build, which carries none.
pub fn carried(cache: &Path) -> Result<(Carried, Option<PathBuf>)> {
    let pack = build_driver::sources::embedded();
    if pack.is_empty() {
        return Ok((Carried::new(), None));
    }
    let u = build_driver::sources::unpack(pack, cache)?;
    let lock = lock::load(&u.dir.join("tools.lock")).unwrap_or_default();
    let mut out = Carried::new();
    for (name, dir) in &u.tools {
        if let Ok(t) = lock::named(&lock, name, system()) {
            out.insert(name.clone(), (dir.clone(), t.sha256.clone()));
        }
    }
    Ok((out, Some(u.dir)))
}

/// Where each of the source's tools for this system comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum From {
    /// The running builder's own copy (the same SHA-256).
    Carried(PathBuf),
    /// Unpacked in the cache already, by its SHA-256.
    Cached(PathBuf),
    /// To download (its size) and unpack there.
    Download(PathBuf, u64),
}

/// Where each of `tools` (the source's lock's lines for this system) comes from, in their order.
pub fn plan(tools: &[Tool], carried: &Carried, store: &Path) -> Vec<(Tool, From)> {
    tools
        .iter()
        .map(|t| {
            let from = match carried.get(&t.name) {
                Some((dir, sha)) if *sha == t.sha256 && dir.is_dir() => From::Carried(dir.clone()),
                _ => {
                    let dir = store.join(format!("{}-{}", t.name, &t.sha256[..16]));
                    if dir.is_dir() { From::Cached(dir) } else { From::Download(dir, t.size) }
                }
            };
            (t.clone(), from)
        })
        .collect()
}

/// The source's tools for this system (its `tools.lock` at `src`), each at its dest under `src`: the bytes
/// downloaded. `fs` is from-source's own cache folder; `progress` is told how far it is.
pub fn install(src: &Path, carried: &Carried, fs: &Path, progress: &dyn Fn(f64)) -> Result<(u64, Vec<String>)> {
    let path = src.join("tools.lock");
    let all = lock::load(&path).map_err(|e| Error::player("source_unreadable", format!("The source's {e}")))?;
    let mine: Vec<Tool> = all.into_iter().filter(|t| t.system == system()).collect();
    let store = fs.join("tools");
    let downloads = fs.join("tool-downloads");
    let (mut downloaded, mut said) = (0, Vec::new());
    let n = mine.len().max(1) as f64;
    for (i, (t, from)) in plan(&mine, carried, &store).into_iter().enumerate() {
        let dir = match from {
            From::Carried(dir) => {
                said.push(format!("{}: the running builder's own ({})", t.name, &t.sha256[..12]));
                dir
            }
            From::Cached(dir) => {
                said.push(format!("{}: kept in the cache ({})", t.name, &t.sha256[..12]));
                dir
            }
            From::Download(dir, size) => {
                let get: archive::Getter<'_> = &build_driver::tools::store::download;
                let archive = archive::fetch(&t, &downloads, get).map_err(|e| {
                    failed(format!("The build tool {} could not be downloaded ({e}). Check the connection.", t.name))
                })?;
                archive::unpack(&archive, &t.prefix, &dir).map_err(|e| {
                    failed(format!(
                        "The build tool {} could not be unpacked ({e}). It is not the running builder's own: unpacking \
                         it takes the zstd program, on the PATH or named by ZSTD.",
                        t.name
                    ))
                })?;
                let _ = std::fs::remove_file(&archive);
                downloaded += size;
                said.push(format!("{}: downloaded, {} MB ({})", t.name, size >> 20, &t.sha256[..12]));
                dir
            }
        };
        let dest = src.join(&t.dest);
        if dest.exists() {
            build_core::fsx::remove_tree(&dest)?;
        }
        build_driver::sources::link_dir(&dir, &dest)?;
        let _ = std::fs::write(dir.join(build_driver::sources::USED), b"");
        progress((i as f64 + 1.0) / n);
    }
    Ok((downloaded, said))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, sha: char) -> Tool {
        let mut t = Tool {
            name: name.into(),
            version: "1".into(),
            system: system().into(),
            sha256: sha.to_string().repeat(64),
            size: 7,
            dest: format!("tools/{name}"),
            prefix: "-".into(),
            urls: vec![],
        };
        t.urls = t.store_urls();
        lock::parse(&t.line()).unwrap().remove(0)
    }

    #[test]
    fn a_tool_the_builder_carries_with_the_same_hash_is_taken_and_others_kept_by_theirs() {
        let d = crate::from_source::source::tests::scratch("tools-plan");
        let (llvm, cpm, sdl) = (tool("llvm", 'a'), tool("cpm", 'b'), tool("sdl2", 'c'));
        std::fs::create_dir_all(d.join("carried/llvm")).unwrap();
        std::fs::create_dir_all(d.join("carried/cpm")).unwrap();
        let carried: Carried = [
            ("llvm".to_string(), (d.join("carried/llvm"), "a".repeat(64))),
            ("cpm".to_string(), (d.join("carried/cpm"), "z".repeat(64))),
        ]
        .into();
        let store = d.join("store");
        std::fs::create_dir_all(store.join(format!("sdl2-{}", "c".repeat(16)))).unwrap();
        let p = plan(&[llvm.clone(), cpm.clone(), sdl.clone()], &carried, &store);
        assert_eq!(p[0], (llvm, From::Carried(d.join("carried/llvm"))), "the same hash: the builder's own");
        assert_eq!(p[1], (cpm, From::Download(store.join(format!("cpm-{}", "b".repeat(16))), 7)), "another hash");
        assert_eq!(p[2], (sdl, From::Cached(store.join(format!("sdl2-{}", "c".repeat(16))))), "kept by its hash");
        std::fs::remove_dir_all(&d).unwrap();
    }
}

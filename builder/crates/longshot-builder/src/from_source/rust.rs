//! The Rust a source pins (pins.rs), installed once in the cache: each part downloaded from static.rust-lang.org,
//! checked by the SHA-256 the source pins, and its components' files merged into one folder as Rust's own install.sh
//! would lay them out (scripts/renhdrum.py's `toolchain`, ported): `<cache>/rust/<version>-<key>/`, `<key>` of the parts'
//! SHA-256s, so another pin for the same version is another folder. Its `.complete` note, written last, names every
//! part; a folder without it (a stopped install) is made again. Each use marks it (`.used`): the cache's budget lets
//! the least recently used go (build_driver::storage, its `rust` kind).

use super::pins::Part;
use build_core::{Error, Result};
use build_driver::tools::archive::Reader;
use sha2::Digest;
use std::io::BufReader;
use std::path::{Path, PathBuf};

/// Where Rust's parts are downloaded from.
pub const DIST: &str = "https://static.rust-lang.org/dist/";
/// The largest part taken (rustc's is about 85 MB).
pub const MAX_PART: u64 = 512 << 20;
/// The note written last in an installed Rust's folder.
pub const COMPLETE: &str = ".complete";

fn failed(text: impl Into<String>) -> Error {
    Error::player("toolchain_download_failed", text)
}

/// The folder `parts` of Rust `version` install into under `rust` (the cache's `rust/`).
pub fn folder(rust: &Path, version: &str, parts: &[Part]) -> PathBuf {
    let key = sha2::Sha256::digest(parts.iter().map(|p| p.sha256.as_str()).collect::<String>().as_bytes());
    rust.join(format!("{version}-{}", &build_core::key::to_hex(&key)[..12]))
}

fn note(parts: &[Part]) -> String {
    parts.iter().map(|p| format!("{} {}\n", p.name, p.sha256)).collect()
}

/// Whether the Rust at `dir` is installed whole, from these parts.
pub fn installed(dir: &Path, parts: &[Part]) -> bool {
    std::fs::read_to_string(dir.join(COMPLETE)).is_ok_and(|n| n == note(parts))
}

/// `parts` of Rust `version` installed under `rust` unless they are: the folder, and the bytes downloaded now.
/// `progress` is told how far it is (0 to 1).
pub fn install(rust: &Path, version: &str, parts: &[Part], progress: &dyn Fn(f64)) -> Result<(PathBuf, u64)> {
    let dir = folder(rust, version, parts);
    let mut downloaded = 0;
    if !installed(&dir, parts) {
        let downloads = rust.join("downloads");
        let part_dir = PathBuf::from(format!("{}.part-{}", dir.display(), std::process::id()));
        let _ = build_core::fsx::remove_tree(&part_dir);
        std::fs::create_dir_all(&part_dir).map_err(|e| Error::io(&part_dir, e))?;
        let n = parts.len() as f64;
        let made = (|| -> Result<()> {
            for (i, p) in parts.iter().enumerate() {
                let archive = downloads.join(format!("{}.tar.xz", p.name));
                downloaded += fetch(p, &archive)?;
                progress((i as f64 + 0.6) / n);
                merge(&archive, &p.name, &part_dir)?;
                progress((i as f64 + 1.0) / n);
            }
            build_core::fsx::write_atomic(&part_dir.join(COMPLETE), note(parts).as_bytes())
        })();
        if let Err(e) = made {
            let _ = build_core::fsx::remove_tree(&part_dir);
            return Err(e);
        }
        if dir.exists() {
            build_core::fsx::remove_tree(&dir)?;
        }
        build_core::fsx::rename(&part_dir, &dir).map_err(|e| Error::io(&dir, e))?;
        // (the downloads are in the installed folder now)
        let _ = build_core::fsx::remove_tree(&downloads);
    }
    let _ = std::fs::write(dir.join(build_driver::sources::USED), b"");
    progress(1.0);
    Ok((dir, downloaded))
}

/// The part's archive at `to`, downloaded unless it is there with its pinned SHA-256: the bytes downloaded.
fn fetch(p: &Part, to: &Path) -> Result<u64> {
    let sha = |f: &Path| build_driver::sha256_file(f).ok();
    if sha(to).is_some_and(|(s, _)| s == p.sha256) {
        return Ok(0);
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    let part = PathBuf::from(format!("{}.part", to.display()));
    let url = format!("{DIST}{}.tar.xz", p.name);
    let got = build_driver::tools::store::download(&url, &part, MAX_PART)
        .and_then(|()| build_driver::sha256_file(&part))
        .and_then(|(s, n)| {
            if s == p.sha256 { Ok(n) } else { Err(format!("{url}: SHA-256 {s}, not the {} the source pins", p.sha256)) }
        });
    match got {
        Ok(n) => {
            std::fs::rename(&part, to).map_err(|e| Error::io(to, e))?;
            Ok(n)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(failed(format!("Rust's {} could not be downloaded ({e}). Check the connection and try again.", p.name)))
        }
    }
}

/// A part's archive (`<name>/<component>/<files>`, its `components` naming them) merged into `prefix`: each
/// component's files at their place there, its `manifest.in` left out; plain files and folders only, nothing outside.
pub fn merge(archive: &Path, name: &str, prefix: &Path) -> Result<()> {
    let bad = |e: String| Error::Format { what: format!("Rust's {name}"), detail: e };
    let file = std::fs::File::open(archive).map_err(|e| Error::io(archive, e))?;
    let (rx, mut tx) = std::io::pipe().map_err(|e| Error::io(archive, e))?;
    std::thread::scope(|s| {
        // (the xz decoded on a thread of its own, into the pipe the tar is read from)
        let decode = s.spawn(move || lzma_rs::xz_decompress(&mut BufReader::with_capacity(1 << 20, file), &mut tx));
        let mut t = Reader::new(BufReader::with_capacity(1 << 20, rx));
        let read = (|| -> Result<()> {
            while let Some(e) = t.next().map_err(bad)? {
                let rest = e.name.split('/').skip(2).collect::<Vec<_>>().join("/");
                let top = e.name.split('/').next() == Some(name);
                if e.kind == b'5' || rest.is_empty() || rest == "manifest.in" {
                    if !top {
                        return Err(bad(format!("{} is not under {name}/", e.name)));
                    }
                    t.skip(e.size).map_err(bad)?;
                    continue;
                }
                if e.kind != b'0' || !top || !build_core::fsx::relative_ok(&rest) {
                    return Err(bad(format!("{}: not a file inside {name}/", e.name)));
                }
                let out = prefix.join(&rest);
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(|err| Error::io(parent, err))?;
                }
                let mut f = std::fs::File::create(&out).map_err(|err| Error::io(&out, err))?;
                t.copy_to(e.size, &mut f).map_err(bad)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = if e.mode & 0o111 != 0 { 0o755 } else { 0o644 };
                    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))
                        .map_err(|err| Error::io(&out, err))?;
                }
            }
            Ok(())
        })();
        // (read to the end, the tar's last records too, so a whole archive's decoder ends well; or, refused, closed: the
        // decoder then stops on its broken pipe)
        if read.is_ok() {
            let _ = std::io::copy(&mut t.r, &mut std::io::sink());
        }
        drop(t);
        let decoded = decode.join().map_err(|_| bad("the xz decoder stopped".into()))?;
        read?;
        decoded.map_err(|e| bad(format!("not an xz archive ({e})")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::from_source::source::tests::{Item, scratch};

    fn tar_xz(items: &[Item<'_>]) -> Vec<u8> {
        let gz = crate::from_source::source::tests::tar_gz(items);
        let mut tar = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&gz[..]), &mut tar).unwrap();
        let mut out = Vec::new();
        lzma_rs::xz_compress(&mut &tar[..], &mut out).unwrap();
        out
    }

    #[test]
    fn a_part_s_components_are_merged_without_their_manifests() {
        let d = scratch("rust-merge");
        let a = d.join("p.tar.xz");
        let items: &[Item<'_>] = &[
            ("rustc-1-x", b'5', b"", 0o755),
            ("rustc-1-x/components", b'0', b"rustc\nrust-std-x\n", 0o644),
            ("rustc-1-x/install.sh", b'0', b"#!/bin/sh\n", 0o755),
            ("rustc-1-x/rustc/manifest.in", b'0', b"file:bin/rustc\n", 0o644),
            ("rustc-1-x/rustc/bin/rustc", b'0', b"\x7fELF", 0o755),
            ("rustc-1-x/rust-std-x/lib/rustlib/x/lib/libstd.rlib", b'0', b"rlib", 0o644),
        ];
        std::fs::write(&a, tar_xz(items)).unwrap();
        let prefix = d.join("prefix");
        merge(&a, "rustc-1-x", &prefix).unwrap();
        assert_eq!(std::fs::read(prefix.join("bin/rustc")).unwrap(), b"\x7fELF");
        assert_eq!(std::fs::read(prefix.join("lib/rustlib/x/lib/libstd.rlib")).unwrap(), b"rlib");
        assert!(!prefix.join("manifest.in").exists() && !prefix.join("install.sh").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(prefix.join("bin/rustc")).unwrap().permissions().mode() & 0o777, 0o755);
        }
        for bad in [
            vec![("rustc-1-x/rustc/bin/../../x", b'0', &b"x"[..], 0o644)],
            vec![("rustc-1-x/rustc/bin/l", b'2', b"/etc/passwd", 0o777)],
            vec![("other/rustc/bin/x", b'0', b"x", 0o644)],
        ] {
            std::fs::write(&a, tar_xz(&bad)).unwrap();
            assert!(merge(&a, "rustc-1-x", &d.join("bad")).is_err(), "{:?}", bad[0].0);
        }
        std::fs::write(&a, b"not xz").unwrap();
        assert!(merge(&a, "rustc-1-x", &d.join("bad")).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_installed_rust_is_taken_by_its_note_alone() {
        let d = scratch("rust-installed");
        let parts = vec![Part { name: "rustc-1-x".into(), sha256: "a".repeat(64) }];
        let dir = folder(&d, "1", &parts);
        assert!(dir.file_name().unwrap().to_string_lossy().starts_with("1-"));
        let other = vec![Part { name: "rustc-1-x".into(), sha256: "b".repeat(64) }];
        assert_ne!(folder(&d, "1", &other), dir, "another pin, another folder");
        assert!(!installed(&dir, &parts));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(COMPLETE), note(&parts)).unwrap();
        assert!(installed(&dir, &parts) && !installed(&dir, &other));
        // (installed: nothing downloaded, the folder marked used)
        let (got, n) = install(&d, "1", &parts, &|_| {}).unwrap();
        assert_eq!((got, n), (dir.clone(), 0));
        assert!(dir.join(build_driver::sources::USED).is_file());
        std::fs::remove_dir_all(&d).unwrap();
    }
}

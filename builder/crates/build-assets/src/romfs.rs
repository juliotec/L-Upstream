//! Where the game's files are read from: the player's romfs, behind [`Romfs`].
//!
//! The builder reads the romfs out of the player's .nup through its index (the game-files crate's
//! `romfs::Index`); the finish stage takes anything that can list, read and fingerprint the files, so it does not
//! depend on how they are stored. [`Folder`] is an unpacked romfs (a developer's dump, the tests).

use build_core::{Error, IoContext, Result, fsx};
use std::path::{Path, PathBuf};

/// The game's files.
pub trait Romfs: Sync {
    /// Every file's path under the romfs, `/`-separated, sorted.
    fn files(&self) -> Result<Vec<String>>;

    /// A file's bytes, by its path under the romfs.
    fn read(&self, rel: &str) -> Result<Vec<u8>>;

    /// What a cache key holds of a file: a string that changes whenever its contents do (a hash of them, or where
    /// they are in a .nup that never changes). Cheap: it is asked for every file an output is made from.
    fn fingerprint(&self, rel: &str) -> Result<String>;

    /// Whether the file is there.
    fn has(&self, rel: &str) -> bool {
        self.files().is_ok_and(|f| f.binary_search_by(|n| n.as_str().cmp(rel)).is_ok())
    }

    /// The files under the folder `dir` (`/`-separated, no trailing `/`), sorted.
    fn files_under(&self, dir: &str) -> Result<Vec<String>> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        Ok(self.files()?.into_iter().filter(|f| f.starts_with(&prefix)).collect())
    }
}

/// An unpacked romfs: a folder.
#[derive(Debug)]
pub struct Folder {
    root: PathBuf,
    files: Vec<String>,
}

impl Folder {
    /// The romfs in `root`.
    pub fn new(root: &Path) -> Result<Folder> {
        if !root.is_dir() {
            return Err(Error::player("no_romfs", format!("{} is not a folder of the game's files", root.display())));
        }
        let mut files: Vec<String> =
            fsx::files_under(root)?.into_iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect();
        files.sort();
        Ok(Folder { root: root.to_path_buf(), files })
    }

    /// A romfs with no files (for the steps that read none).
    pub fn empty() -> Folder {
        Folder { root: PathBuf::new(), files: Vec::new() }
    }

    fn path(&self, rel: &str) -> PathBuf {
        let mut p = self.root.clone();
        p.extend(rel.split('/'));
        p
    }
}

impl Romfs for Folder {
    fn files(&self) -> Result<Vec<String>> {
        Ok(self.files.clone())
    }

    fn read(&self, rel: &str) -> Result<Vec<u8>> {
        let p = self.path(rel);
        std::fs::read(&p).at(&p)
    }

    fn fingerprint(&self, rel: &str) -> Result<String> {
        crate::cache::sha1_file(&self.path(rel))
    }

    fn has(&self, rel: &str) -> bool {
        self.files.binary_search_by(|n| n.as_str().cmp(rel)).is_ok()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A romfs in memory, for tests.
    #[derive(Default)]
    pub(crate) struct Memory(pub(crate) BTreeMap<String, Vec<u8>>);

    impl Memory {
        pub(crate) fn with(mut self, rel: &str, bytes: Vec<u8>) -> Memory {
            self.0.insert(rel.into(), bytes);
            self
        }
    }

    impl Romfs for Memory {
        fn files(&self) -> Result<Vec<String>> {
            Ok(self.0.keys().cloned().collect())
        }
        fn read(&self, rel: &str) -> Result<Vec<u8>> {
            self.0.get(rel).cloned().ok_or_else(|| Error::format("romfs", format!("no file {rel}")))
        }
        fn fingerprint(&self, rel: &str) -> Result<String> {
            Ok(build_core::key::sha1_hex(&self.read(rel)?))
        }
    }

    #[test]
    fn a_folder_lists_its_files_by_romfs_path() {
        let d = std::env::temp_dir().join(format!("build-assets-romfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        fsx::write_atomic(&d.join("Mals/b.zs"), b"b").unwrap();
        fsx::write_atomic(&d.join("Pack/a"), b"a").unwrap();
        let f = Folder::new(&d).unwrap();
        assert_eq!(f.files().unwrap(), ["Mals/b.zs", "Pack/a"]);
        assert_eq!(f.files_under("Mals").unwrap(), ["Mals/b.zs"]);
        assert!(f.has("Pack/a") && !f.has("Pack"));
        assert_eq!(f.read("Pack/a").unwrap(), b"a");
        assert_eq!(f.fingerprint("Pack/a").unwrap(), build_core::key::sha1_hex(b"a"));
        std::fs::remove_dir_all(&d).unwrap();
    }
}

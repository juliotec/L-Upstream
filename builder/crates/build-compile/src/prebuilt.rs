//! The third-party code's objects, compiled by the release (`third_party_objects` in the builder's tools), so a
//! player's first build does not compile them: Eden's shader recompiler and sirit, Dear ImGui, zbic's zstd, ReShade's
//! FX compiler bar its adapted preprocessor (the renderer), libopus and zstd's decoder (the runtime). Nothing in it is
//! the renderer's own code, the runtime's, or made from the game.
//!
//! A set is a [`Store`] (its objects and their manifests) with `set.json` beside it, each object's SHA-1 and size: an
//! object is taken only by its key ([`crate::key`]: the compiler's identity, the flags, the source, every header it
//! read), so a set made with another compiler, other headers or other sources simply has no object of this build's
//! keys, and each is checked against `set.json` as it is copied into the store, so a damaged file is compiled instead.
//!
//! The third-party units are built for x86-64-v3 on every CPU that has it, not `-march=native`
//! ([`crate::toolchain::Toolchain::third_party_cpu`]), so that a player's keys are the release's.

use crate::plan::Unit;
use crate::store::{self, Entry, Store};
use build_core::{Error, IoContext, Result, fsx};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The set's description file.
pub const FILE: &str = "set.json";
/// Its format.
const FORMAT: u32 = 1;

/// One object of a set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Held {
    /// Its SHA-1.
    pub sha1: String,
    /// Its size.
    pub size: u64,
    /// Its source, relative to the tree.
    pub source: String,
}

#[derive(Serialize, Deserialize)]
struct SetFile {
    format: u32,
    objects: BTreeMap<String, Held>,
}

/// A prebuilt set, read.
pub struct Prebuilt {
    store: Store,
    objects: BTreeMap<String, Held>,
}

impl Prebuilt {
    /// The set at `dir`, or `None` (none there, unreadable, another format).
    pub fn open(dir: &Path) -> Option<Prebuilt> {
        let f: SetFile = serde_json::from_slice(&std::fs::read(dir.join(FILE)).ok()?).ok()?;
        (f.format == FORMAT).then(|| Prebuilt { store: Store::at(dir), objects: f.objects })
    }

    /// How many objects it holds.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Whether the set holds an object for a base whose headers are as they were (nothing copied).
    pub fn holds(&self, base: &str, header: impl FnMut(&str) -> Option<String>) -> bool {
        store::lookup(&self.store.entries(base), header).is_some_and(|k| self.objects.contains_key(&k))
    }

    /// The key a base has in the set while its headers are as they were, its object copied into `into` once checked;
    /// `None` otherwise.
    pub fn take(&self, base: &str, into: &Store, header: impl FnMut(&str) -> Option<String>) -> Option<String> {
        let entries = self.store.entries(base);
        let key = store::lookup(&entries, header)?;
        let held = self.objects.get(&key)?;
        if into.has(&key) {
            return Some(key);
        }
        let from = self.store.object(&key);
        let data = std::fs::read(&from).ok()?;
        // (a copy, checked: a release's unpacked tree is shared, and a damaged file is compiled instead)
        if data.len() as u64 != held.size || sha1_smol::Sha1::from(&data).digest().to_string() != held.sha1 {
            return None;
        }
        let tmp = into.object(&key).with_extension(format!("o.{}.in", std::process::id()));
        fsx::write_atomic(&tmp, &data).ok()?;
        let put = into.put(&key, &tmp);
        let _ = std::fs::remove_file(&tmp);
        put.ok()?;
        let entry = entries.into_iter().find(|e| e.key == key)?;
        into.add_entry(base, &entry).ok()?;
        Some(key)
    }
}

/// The set written at `out` (emptied first) from the units' objects in `from`: `(unit, base, key)` each. The
/// description last, so a set cut short is no set.
pub fn write(
    out: &Path,
    root: &Path,
    from: &Store,
    made: &[(&Unit, String, String)],
) -> Result<BTreeMap<String, Held>> {
    fsx::remove_tree(out)?;
    let set = Store::at(out);
    let mut objects = BTreeMap::new();
    for (u, base, key) in made {
        let obj = from.object(key);
        let data = std::fs::read(&obj).at(&obj)?;
        set.put(key, &obj)?;
        let entry = from
            .entries(base)
            .into_iter()
            .find(|e| &e.key == key)
            .unwrap_or(Entry { key: key.clone(), headers: Vec::new() });
        set.add_entry(base, &entry)?;
        let source = u.source.strip_prefix(root).unwrap_or(&u.source).to_string_lossy().replace('\\', "/");
        objects.insert(
            key.clone(),
            Held { sha1: sha1_smol::Sha1::from(&data).digest().to_string(), size: data.len() as u64, source },
        );
    }
    let text = serde_json::to_vec_pretty(&SetFile { format: FORMAT, objects: objects.clone() })
        .map_err(|e| Error::format("prebuilt set", e.to_string()))?;
    fsx::write_atomic(&out.join(FILE), &text)?;
    Ok(objects)
}

/// The set a build uses: `LONGSHOT_THIRD_PARTY_OBJECTS`'s folder, else the tools' own (`<root>/third_party_objects`);
/// `None` when it is turned off.
pub fn folder(setting: &Option<Option<PathBuf>>, root: &Path) -> Option<PathBuf> {
    match setting {
        Some(None) => None,
        Some(Some(p)) => Some(p.clone()),
        None => Some(root.join("third_party_objects")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_object_is_not_taken() {
        let d = crate::testdir("prebuilt");
        let set_dir = d.join("set");
        let made = Store::at(d.join("made"));
        let o = d.join("x.o");
        std::fs::write(&o, b"object").unwrap();
        made.put("ab01", &o).unwrap();
        made.add_entry("ba01", &Entry { key: "ab01".into(), headers: vec![] }).unwrap();
        let u = Unit {
            name: "x.c".into(),
            module: crate::plan::Module::Game,
            kind: crate::plan::Kind::ThirdParty,
            compiler: "cc".into(),
            source: d.join("x.c"),
            object: d.join("x.o"),
            flags: vec![],
            extra: vec![],
            depfile: true,
            third_party: true,
            listed: None,
        };
        write(&set_dir, &d, &made, &[(&u, "ba01".into(), "ab01".into())]).unwrap();
        let set = Prebuilt::open(&set_dir).unwrap();
        assert_eq!(set.len(), 1);
        let player = Store::at(d.join("player"));
        assert_eq!(set.take("ba01", &player, |_| None), Some("ab01".into()));
        assert!(player.has("ab01"));
        // (damaged: another file in its place)
        let other = Store::at(d.join("other"));
        std::fs::remove_file(set.store.object("ab01")).unwrap();
        std::fs::write(set.store.object("ab01"), b"objecT").unwrap();
        assert_eq!(set.take("ba01", &other, |_| None), None);
    }
}

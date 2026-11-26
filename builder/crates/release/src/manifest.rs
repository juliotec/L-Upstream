//! A release's manifest for one platform, `longshot-builder-<v>-<platform>.json`, signed in its `SHA256SUMS`
//! (longshot-builder's dev/release_deltas.rs writes it; the Longshot Launcher's LongshotBuildRelease.Parse and Choose,
//! ported):
//!
//! ```text
//! {"v": 1, "version", "channel", "platform", "full": {"name", "size", "sha256"},
//!  "deltas": [{"from", "from_sha256", "name", "size", "sha256", "long"}], "stamps": {"compiler", "translator"}}
//! ```
//!
//! A release has one when it is of longshot-builder's kind; any other (a make_game.py release) has none.

use crate::Channel;
use serde_json::Value;
use std::collections::BTreeMap;

/// The manifest's format read.
pub const FORMAT: u64 = 1;
/// The largest manifest read.
pub const MAX_MANIFEST: u64 = 1 << 20;

/// The manifest's name for a version and platform.
#[must_use]
pub fn manifest_name(version: &str, platform: &str) -> String {
    format!("longshot-builder-{version}-{platform}.json")
}

/// A file the manifest names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Its plain name in the release.
    pub name: String,
    /// Its size.
    pub size: u64,
    /// Its SHA-256 (lowercase hex).
    pub sha256: String,
}

/// A delta the manifest lists: applied to `from`'s full file (its SHA-256 `from_sha256`), it gives this release's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delta {
    /// The version it was made from.
    pub from: String,
    /// That version's full file's SHA-256.
    pub from_sha256: String,
    /// The delta file itself.
    pub file: FileEntry,
}

/// A release's manifest, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The release.
    pub version: String,
    /// Its channel.
    pub channel: Channel,
    /// The platform (`linux-x64`, `windows-x64`).
    pub platform: String,
    /// The full file.
    pub full: FileEntry,
    /// Its deltas, in the manifest's order.
    pub deltas: Vec<Delta>,
    /// The stamps a launcher keys shader caches by (`compiler`, `translator`).
    pub stamps: BTreeMap<String, String>,
}

fn sha(v: Option<&Value>) -> Result<String, String> {
    match v.and_then(Value::as_str) {
        Some(s) if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(s.to_ascii_lowercase()),
        _ => Err("a SHA-256 that is not one".into()),
    }
}

fn entry(v: &Value) -> Result<FileEntry, String> {
    let name = v.get("name").and_then(Value::as_str).unwrap_or("");
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err("a file's name that is not a plain name".into());
    }
    let size = v.get("size").and_then(Value::as_u64).ok_or("a file with no size")?;
    if size == 0 || size > crate::forge::MAX_FILE {
        return Err("a size out of range".into());
    }
    Ok(FileEntry { name: name.into(), size, sha256: sha(v.get("sha256"))? })
}

impl Manifest {
    /// The manifest of `version` for `platform` in `json`, checked: its format, that it is this version's, of its
    /// channel, this platform's, its files plain names with sizes and SHA-256s.
    pub fn parse(json: &str, version: &str, platform: &str) -> Result<Manifest, String> {
        let refused = |why: String| format!("The manifest of longshot-builder {version} is refused: {why}.");
        let v: Value = serde_json::from_str(json)
            .map_err(|e| format!("The manifest of longshot-builder {version} can't be read: {e}"))?;
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        if !v.is_object() {
            return Err(refused("not an object".into()));
        }
        if v.get("v").and_then(Value::as_u64) != Some(FORMAT) {
            return Err(refused(format!("its format is not {FORMAT}")));
        }
        if s("version") != Some(version) {
            return Err(refused(format!("it is not {version}'s")));
        }
        if s("platform") != Some(platform) {
            return Err(refused(format!("it is not {platform}'s")));
        }
        let channel = Channel::of(version).ok_or_else(|| refused(format!("{version} is on no channel")))?;
        if s("channel") != Some(channel.name()) {
            return Err(refused(format!("its channel, {}, is not {version}'s", s("channel").unwrap_or("none"))));
        }
        let full = entry(v.get("full").ok_or_else(|| refused("it names no full file".into()))?).map_err(refused)?;
        let mut deltas = Vec::new();
        for d in v.get("deltas").and_then(Value::as_array).into_iter().flatten() {
            let file = entry(d).map_err(refused)?;
            let from = d
                .get("from")
                .and_then(Value::as_str)
                .filter(|f| crate::versions::is_tag_name(f) && Channel::of(f).is_some())
                .ok_or_else(|| refused("a delta's from is no version".into()))?;
            deltas.push(Delta { from: from.into(), from_sha256: sha(d.get("from_sha256")).map_err(refused)?, file });
        }
        let stamps = v
            .get("stamps")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str().filter(|s| !s.is_empty())?.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Manifest { version: version.into(), channel, platform: platform.into(), full, deltas, stamps })
    }

    /// The delta to download: the first the manifest lists whose base `base` has (a kept file with its SHA-256, the
    /// one the delta was made from); None: none fits.
    #[must_use]
    pub fn choose(&self, base: &dyn Fn(&str, &str) -> bool) -> Option<&Delta> {
        self.deltas.iter().find(|d| base(&d.from, &d.from_sha256))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sha_of(c: char) -> String {
        std::iter::repeat_n(c, 64).collect()
    }

    pub fn manifest_json(version: &str, platform: &str, full_size: u64, deltas: &[(&str, char, &str, u64)]) -> String {
        let channel = Channel::of(version).map_or("none", Channel::name);
        let ds: Vec<Value> = deltas
            .iter()
            .map(|(from, from_sha, name, size)| {
                serde_json::json!({"from": from, "from_sha256": sha_of(*from_sha), "name": name, "size": size,
                                   "sha256": sha_of('d'), "long": 30})
            })
            .collect();
        serde_json::json!({"v": 1, "version": version, "channel": channel, "platform": platform,
            "full": {"name": format!("longshot-builder-{version}-{platform}"), "size": full_size, "sha256": sha_of('f')},
            "deltas": ds, "stamps": {"compiler": "c", "translator": "t", "empty": ""}})
        .to_string()
    }

    #[test]
    fn a_manifest_is_read_and_checked() {
        let j = manifest_json(
            "v3.1.0",
            "linux-x64",
            1000,
            &[("v3.0.0", 'a', "d1", 10), ("nightly-5-aaaaaaaa", 'b', "d2", 20)],
        );
        let m = Manifest::parse(&j, "v3.1.0", "linux-x64").unwrap();
        assert_eq!(m.channel, Channel::Stable);
        assert_eq!(m.full.size, 1000);
        assert_eq!(m.deltas.len(), 2);
        assert_eq!(m.stamps.len(), 2, "an empty stamp is left out");
        // (another version's, platform's, a bad format, a channel that is not its own)
        assert!(Manifest::parse(&j, "v3.2.0", "linux-x64").unwrap_err().contains("not v3.2.0's"));
        assert!(Manifest::parse(&j, "v3.1.0", "windows-x64").unwrap_err().contains("windows-x64"));
        let mut v: Value = serde_json::from_str(&j).unwrap();
        v["v"] = 2.into();
        assert!(Manifest::parse(&v.to_string(), "v3.1.0", "linux-x64").unwrap_err().contains("format"));
        let mut v: Value = serde_json::from_str(&j).unwrap();
        v["channel"] = "nightly".into();
        assert!(Manifest::parse(&v.to_string(), "v3.1.0", "linux-x64").unwrap_err().contains("channel"));
        let mut v: Value = serde_json::from_str(&j).unwrap();
        v["deltas"][0]["name"] = "../x".into();
        assert!(Manifest::parse(&v.to_string(), "v3.1.0", "linux-x64").unwrap_err().contains("plain name"));
        let mut v: Value = serde_json::from_str(&j).unwrap();
        v["deltas"][0]["from"] = "main".into();
        assert!(Manifest::parse(&v.to_string(), "v3.1.0", "linux-x64").unwrap_err().contains("no version"));
        let mut v: Value = serde_json::from_str(&j).unwrap();
        v["full"]["sha256"] = "xyz".into();
        assert!(Manifest::parse(&v.to_string(), "v3.1.0", "linux-x64").unwrap_err().contains("SHA-256"));
        let mut v: Value = serde_json::from_str(&j).unwrap();
        v["full"]["size"] = 0.into();
        assert!(Manifest::parse(&v.to_string(), "v3.1.0", "linux-x64").unwrap_err().contains("size"));
        assert!(Manifest::parse("[", "v3.1.0", "linux-x64").unwrap_err().contains("can't be read"));
    }

    #[test]
    fn the_first_delta_whose_base_is_kept_is_chosen() {
        let j = manifest_json("v3.1.0", "linux-x64", 1000, &[("v3.0.0", 'a', "d1", 10), ("v3.0.1", 'b', "d2", 20)]);
        let m = Manifest::parse(&j, "v3.1.0", "linux-x64").unwrap();
        assert_eq!(m.choose(&|_, sha| sha == sha_of('b')).map(|d| d.file.name.as_str()), Some("d2"));
        assert_eq!(m.choose(&|_, _| true).map(|d| d.file.name.as_str()), Some("d1"));
        assert!(m.choose(&|_, sha| sha == sha_of('c')).is_none(), "a kept file of another SHA-256 is no base");
    }
}

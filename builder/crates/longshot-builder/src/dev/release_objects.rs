//! `longshot-builder dev release last-objects`: the third-party objects of the channel's newest release, to seed this
//! release's (docs/dev/reference/releases.md, "The third-party objects"). The release's `SHA256SUMS` is checked against
//! its `SHA256SUMS.sig` with the keys the builder trusts (the release crate's trust.rs, as a player's update checks it)
//! before a byte of its full file is taken; the full file is checked by the SHA-256 the signed sums give, its pack
//! found (appended after the link, or compiled in by a release from before: build_driver::sources::find_pack), and
//! only its `tree/third_party_objects` member written out, each file checked. `compile --third-party-objects` then
//! takes each object of that set whose key (the compiler's identity, the flags, the source, every header it read) is
//! this build's, so the objects that did not change are the last release's bytes and the rest are compiled. The whole
//! pack is kept beside them ([`PACK`]), for `dev release sources --reuse`: each member whose key (its raw bytes, the
//! zstd program and its arguments) is this build's is taken from it as it is, not compressed again.
//!
//! Nothing here stops a release: whatever is missing or refused is said (`::warning::` when it is refused), and the
//! objects are all compiled, as before.

use super::release_deltas::{self as rd, GetFile};
use release::trust::{self, Key};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The member of the pack the third-party objects are in (build_driver::sources::group's).
pub const MEMBER: &str = "tree/third_party_objects";
/// The file in `--out` the release's whole pack is kept in, for `dev release sources --reuse` (the members that did not
/// change are taken from it, not compressed again: build_driver::sources::write_with).
pub const PACK: &str = "sources.pack";
/// How many of the channel's newest releases are tried, newest first.
const TRIES: usize = 3;

/// A URL's bytes, at most so many.
pub type GetBytes<'a> = &'a dyn Fn(&str, u64) -> Result<Vec<u8>, String>;

/// How the network is reached: a URL's JSON, a URL's bytes (at most so many), a file downloaded and checked.
pub struct Net<'a> {
    pub json: &'a dyn Fn(&str) -> Result<Value, String>,
    pub bytes: GetBytes<'a>,
    pub file: GetFile<'a>,
}

/// What [`last_objects`] looks for.
pub struct Last<'a> {
    pub platform: &'a str,
    /// This release's tag: its channel's releases are looked at, never itself.
    pub version: &'a str,
    pub out: &'a Path,
    pub api: &'a str,
    pub repo: &'a str,
}

/// The third-party objects of the channel's newest release with this platform's files, checked as the module says,
/// in `<out>/objects/third_party_objects` (a set, its `set.json` in it): (that release's tag, the set's folder). None,
/// said, when no release gives them.
pub fn last_objects(
    l: &Last<'_>,
    net: &Net<'_>,
    keys: &[Key],
    say: &mut dyn FnMut(String),
) -> Option<(String, PathBuf)> {
    let Some(channel) = rd::channel_of(l.version) else {
        say(format!("{} is on no channel: the third-party objects are compiled", l.version));
        return None;
    };
    let url = format!("{}/repos/{}/releases?draft=false&limit=50&page=1", l.api, l.repo);
    let releases = match (net.json)(&url) {
        Ok(Value::Array(r)) => r,
        Ok(_) => Vec::new(),
        Err(e) => {
            say(format!("::warning::the release list could not be read ({e}): the third-party objects are compiled"));
            return None;
        }
    };
    let full = |tag: &str| rd::full_name(tag, l.platform);
    let mine = releases.iter().filter(|r| {
        let tag = r.get("tag_name").and_then(Value::as_str).unwrap_or("");
        tag != l.version && rd::channel_of(tag) == Some(channel) && asset(r, &full(tag)).is_some()
    });
    for rel in mine.take(TRIES) {
        let tag = rel.get("tag_name").and_then(Value::as_str).unwrap_or("");
        match take(l, net, keys, rel, tag, &full(tag)) {
            Ok((dir, n)) => {
                say(format!("the third-party objects of {tag}: {n} files, checked by its signed SHA256SUMS"));
                return Some((tag.to_string(), dir));
            }
            Err(e) => say(format!("::warning::{tag}'s third-party objects not taken: {e}")),
        }
    }
    say(format!("no {channel} release gives its third-party objects: they are compiled"));
    None
}

/// A release's asset's address and size.
fn asset(rel: &Value, name: &str) -> Option<(String, u64)> {
    let a = rel.get("assets")?.as_array()?.iter().find(|a| a.get("name").and_then(Value::as_str) == Some(name))?;
    let url = a.get("browser_download_url")?.as_str().filter(|u| !u.is_empty())?.to_string();
    Some((url, a.get("size").and_then(Value::as_u64).unwrap_or(0)))
}

/// One release's objects: its signed sums opened, its full file downloaded and checked, the member written out.
fn take(
    l: &Last<'_>,
    net: &Net<'_>,
    keys: &[Key],
    rel: &Value,
    tag: &str,
    full: &str,
) -> Result<(PathBuf, u64), String> {
    let _ = std::fs::remove_file(l.out.join(PACK));
    let need = |name: &str| asset(rel, name).ok_or_else(|| format!("it has no {name}"));
    let (sums_url, sig_url, (full_url, size)) = (need("SHA256SUMS")?.0, need("SHA256SUMS.sig")?.0, need(full)?);
    let sums = (net.bytes)(&sums_url, trust::MAX_SUMS)?;
    let sig = (net.bytes)(&sig_url, trust::MAX_SIG)?;
    let named = trust::open(&sums, &String::from_utf8_lossy(&sig), keys)?;
    let sha = named.get(full).ok_or_else(|| format!("its signed SHA256SUMS names no {full}"))?;
    std::fs::create_dir_all(l.out).map_err(|e| format!("{}: {e}", l.out.display()))?;
    let file = l.out.join(full);
    (net.file)(&full_url, &file, sha, size)?;
    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let pack = build_driver::sources::find_pack(&bytes).ok_or("its program carries no pack this reads")?;
    let into = l.out.join("objects");
    let _ = build_core::fsx::remove_tree(&into);
    let n = build_driver::sources::unpack_member(pack, MEMBER, &into).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&file);
    let dir = into.join("third_party_objects");
    if !dir.join("set.json").is_file() {
        return Err(format!("{tag}'s {MEMBER} holds no set.json"));
    }
    // (the whole pack too, from the file its signed sums named: `dev release sources --reuse` takes the members that did
    // not change)
    let kept = l.out.join(PACK);
    std::fs::write(&kept, pack).map_err(|e| format!("{}: {e}", kept.display()))?;
    Ok((dir, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use build_driver::sources;
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    fn member(files: &[(&str, &[u8])]) -> Vec<u8> {
        let entries: Vec<sources::Entry> = files
            .iter()
            .map(|(p, d)| sources::Entry { path: p.to_string(), mode: 0, body: sources::Body::Bytes(d.to_vec()) })
            .collect();
        ruzstd::encoding::compress_to_vec(
            &sources::member_bytes(&entries).unwrap()[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    }

    /// A release's full file: a program (the test's own, an ELF this reads) with a pack holding a set appended.
    fn full_file() -> Vec<u8> {
        let pack = sources::pack_bytes(&[
            ("tree/files".into(), member(&[("VERSION", b"nightly-1-a\n")])),
            (MEMBER.into(), member(&[("third_party_objects/set.json", b"{}"), ("third_party_objects/ab/cd.o", b"o")])),
        ]);
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        sources::attach(&me, &pack).unwrap()
    }

    struct World {
        files: BTreeMap<String, Vec<u8>>,
        releases: Value,
    }

    fn world(signed_by: &trust::tests::SigningKey, tamper: bool) -> World {
        let full = full_file();
        let sums =
            format!("{}  longshot-builder-nightly-1-a-linux-x64\n", build_core::key::to_hex(&Sha256::digest(&full)));
        let sig = trust::tests::sign(signed_by, sums.as_bytes());
        let mut served = full.clone();
        if tamper {
            *served.last_mut().unwrap() ^= 1;
        }
        let base = "https://forge/files/";
        let assets = |names: &[(&str, usize)]| {
            names
                .iter()
                .map(|(n, size)| serde_json::json!({"name": n, "browser_download_url": format!("{base}{n}"), "size": size}))
                .collect::<Vec<_>>()
        };
        let releases = serde_json::json!([
            {"tag_name": "nightly-2-b", "assets": []},
            {"tag_name": "v1.0.0", "assets": assets(&[("longshot-builder-v1.0.0-linux-x64", 1)])},
            {"tag_name": "nightly-1-a", "assets": assets(&[
                ("longshot-builder-nightly-1-a-linux-x64", full.len()), ("SHA256SUMS", 1), ("SHA256SUMS.sig", 1)])},
        ]);
        let mut files = BTreeMap::new();
        files.insert(format!("{base}longshot-builder-nightly-1-a-linux-x64"), served);
        files.insert(format!("{base}SHA256SUMS"), sums.into_bytes());
        files.insert(format!("{base}SHA256SUMS.sig"), sig.into_bytes());
        World { files, releases }
    }

    fn run(w: &World, keys: &[Key], out: &Path) -> (Option<(String, PathBuf)>, Vec<String>) {
        let json = |_: &str| Ok(w.releases.clone());
        let bytes = |u: &str, _: u64| w.files.get(u).cloned().ok_or_else(|| format!("{u}: 404"));
        let file = |u: &str, to: &Path, sha: &str, size: u64| {
            let b = w.files.get(u).ok_or("404")?;
            if build_core::key::to_hex(&Sha256::digest(b)) != sha || b.len() as u64 != size {
                return Err("other bytes".to_string());
            }
            std::fs::write(to, b).map_err(|e| e.to_string())
        };
        let net = Net { json: &json, bytes: &bytes, file: &file };
        let l = Last { platform: "linux-x64", version: "nightly-3-c", out, api: "https://forge/api/v1", repo: "o/r" };
        let mut said = Vec::new();
        let got = last_objects(&l, &net, keys, &mut |s| said.push(s));
        (got, said)
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ls-last-objects-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_newest_signed_release_s_set_is_taken() {
        let (sk, key) = trust::tests::test_key();
        let out = scratch("taken");
        let (got, said) = run(&world(&sk, false), &[key], &out);
        let (tag, dir) = got.unwrap_or_else(|| panic!("{said:?}"));
        assert_eq!(tag, "nightly-1-a", "the newest nightly with this platform's file, not the stable");
        assert_eq!(std::fs::read(dir.join("set.json")).unwrap(), b"{}");
        assert_eq!(std::fs::read(dir.join("ab/cd.o")).unwrap(), b"o");
        assert!(!out.join("objects/VERSION").exists(), "that member alone");
        let kept = std::fs::read(out.join(PACK)).unwrap();
        assert_eq!(sources::reusable(&kept).unwrap().len(), 2, "and the whole pack kept, for sources --reuse");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn sums_signed_by_another_key_or_a_changed_file_give_nothing() {
        let (sk, _) = trust::tests::key(1);
        let (_, trusted) = trust::tests::key(2);
        let out = scratch("refused");
        let (got, said) = run(&world(&sk, false), &[trusted], &out);
        assert!(got.is_none());
        assert!(said.iter().any(|s| s.contains("not signed by a key")), "{said:?}");
        let (sk2, k2) = trust::tests::key(3);
        let (got, said) = run(&world(&sk2, true), &[k2], &out);
        assert!(got.is_none());
        assert!(said.iter().any(|s| s.contains("other bytes")), "{said:?}");
        assert!(!out.join("objects").exists());
        assert!(!out.join(PACK).exists(), "no pack of a release refused");
        let _ = std::fs::remove_dir_all(&out);
    }
}

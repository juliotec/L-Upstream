//! A chain of deltas, when no single delta of a release fits a file this PC keeps (the Longshot Launcher's
//! Toolchains.ChainAsync and LongshotBuildRelease.LookBack, ported).
//!
//! Each release has deltas from its channel's last [`DELTAS_FROM`] releases (a nightly's from the latest stable too).
//! A chain walks back from the target's manifest a level of hops at a time, through the earlier releases its deltas
//! were made from, each one's manifest read and checked through its own signed SHA256SUMS. A hop counts only when its
//! delta is in its release's files and, for an intermediate release, was made from that release's full file. The
//! chain starts at a kept file whose SHA-256 is the first delta's `from_sha256`. The fewest hops win, then the smallest
//! download; none beyond [`MAX_HOPS`], none not smaller than the full file. At most [`LOOK_BACK`] + 1 manifests are
//! read.

use crate::forge::Listed;
use crate::manifest::{Delta, Manifest};
use crate::{Channel, forge};
use std::collections::{BTreeMap, BTreeSet};

/// The most deltas a chain applies: each hop downloads, patches and checks a builder-sized file, so a longer chain
/// rarely beats the full file.
pub const MAX_HOPS: usize = 5;
/// How many of its channel's last releases a release has deltas from (dev/release_deltas.rs's `KEEP`).
pub const DELTAS_FROM: usize = 5;
/// How many of the channel's releases before the target a chain may pass through: a chain passes through
/// `MAX_HOPS - 1` releases, each at most `DELTAS_FROM` before the one after it.
pub const LOOK_BACK: usize = DELTAS_FROM * (MAX_HOPS - 1);

/// A release's files by name: their links.
pub type Files = BTreeMap<String, String>;

/// One delta of a chain: applied to the file before it, it makes `into`'s full file, checked against it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    /// The release whose full file it makes.
    pub into: Manifest,
    /// The delta.
    pub delta: Delta,
    /// Its download link, in `into`'s release.
    pub url: String,
}

/// The releases a chain to `target` may pass through, newest first: the [`LOOK_BACK`] newest of its channel older
/// than it, and for a nightly the newest stable (a nightly has a delta from it).
#[must_use]
pub fn look_back(list: &[Listed], target: &str) -> Vec<String> {
    let Some(channel) = Channel::of(target) else { return Vec::new() };
    let mut back: Vec<String> = forge::on_channel(list, channel)
        .into_iter()
        .map(|r| r.tag.clone())
        .filter(|t| crate::versions::newer(target, t))
        .take(LOOK_BACK)
        .collect();
    if channel == Channel::Nightly
        && let Some(stable) = forge::on_channel(list, Channel::Stable).first()
        && !back.contains(&stable.tag)
    {
        back.push(stable.tag.clone());
    }
    back
}

/// An earlier release's manifest and files, read and checked (its signed SHA256SUMS), for a chain: None for one the
/// chain may not pass through ([`look_back`]) or that has no manifest for the platform; Err why it can't be read.
pub type Earlier<'a> = dyn FnMut(&str) -> Result<Option<(Manifest, Files)>, String> + 'a;

/// The chain from a kept file (`kept(from, from_sha256)`) to `target` (its release's files `files`), or None (the full
/// file instead). `say`: what was left out, for the log.
pub fn find(
    target: &Manifest,
    files: &Files,
    kept: &dyn Fn(&str, &str) -> bool,
    earlier: &mut Earlier<'_>,
    say: &mut dyn FnMut(String),
) -> Option<Vec<Hop>> {
    struct Level {
        manifest: Manifest,
        files: Files,
        tail: Vec<Hop>,
        size: u64,
    }
    let mut seen = BTreeSet::from([target.version.clone()]);
    let mut level = vec![Level { manifest: target.clone(), files: files.clone(), tail: Vec::new(), size: 0 }];
    let mut read = 0usize;
    for hops in 1..=MAX_HOPS {
        if level.is_empty() {
            break;
        }
        // (each version this level reaches, by its cheapest way on to the target)
        let mut reach: BTreeMap<String, (Vec<Hop>, u64)> = BTreeMap::new();
        for l in &level {
            for d in &l.manifest.deltas {
                let Some(url) = l.files.get(&d.file.name) else { continue };
                if seen.contains(&d.from) {
                    continue;
                }
                let total = l.size + d.file.size;
                if total >= target.full.size || reach.get(&d.from).is_some_and(|(_, s)| *s <= total) {
                    continue;
                }
                let hop = Hop { into: l.manifest.clone(), delta: d.clone(), url: url.clone() };
                let chain: Vec<Hop> = std::iter::once(hop).chain(l.tail.iter().cloned()).collect();
                reach.insert(d.from.clone(), (chain, total));
            }
        }
        let found = reach
            .values()
            .filter(|(c, _)| kept(&c[0].delta.from, &c[0].delta.from_sha256))
            .min_by_key(|(_, size)| *size);
        if let Some((chain, _)) = found {
            return Some(chain.clone());
        }
        if hops == MAX_HOPS {
            break;
        }
        let mut order: Vec<(String, Vec<Hop>, u64)> = reach.into_iter().map(|(f, (c, s))| (f, c, s)).collect();
        order.sort_by_key(|(_, _, s)| *s);
        let mut next = Vec::new();
        for (from, chain, size) in order {
            seen.insert(from.clone());
            if read > LOOK_BACK {
                continue;
            }
            match earlier(&from) {
                Ok(Some((m, f))) => {
                    read += 1;
                    // (the delta was made from that release's full file: else what the chain makes there is no base)
                    if m.full.sha256 != chain[0].delta.from_sha256 {
                        say(format!(
                            "{from}'s full file is not the one its delta to {} was made from",
                            chain[0].into.version
                        ));
                        continue;
                    }
                    next.push(Level { manifest: m, files: f, tail: chain, size });
                }
                Ok(None) => {}
                Err(e) => say(format!("{from}'s manifest is no step of a chain ({e})")),
            }
        }
        level = next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileEntry, Manifest};

    fn sha(n: u32) -> String {
        format!("{n:064x}")
    }

    /// Release `n` (nightly-n), its full file's SHA-256 sha(n), size 1000, with a delta from each of `from` of `size`.
    fn rel(n: u32, from: &[u32], size: u64) -> (Manifest, Files) {
        let version = format!("nightly-{n}-0000000{}", n % 10);
        let deltas = from
            .iter()
            .map(|&f| Delta {
                from: format!("nightly-{f}-0000000{}", f % 10),
                from_sha256: sha(f),
                file: FileEntry { name: format!("d{f}-{n}"), size, sha256: sha(9000 + f) },
            })
            .collect::<Vec<_>>();
        let files = deltas.iter().map(|d| (d.file.name.clone(), format!("https://x/{}", d.file.name))).collect();
        let full = FileEntry { name: format!("full{n}"), size: 1000, sha256: sha(n) };
        (
            Manifest {
                version,
                channel: Channel::Nightly,
                platform: "linux-x64".into(),
                full,
                deltas,
                stamps: BTreeMap::new(),
            },
            files,
        )
    }

    fn tag(n: u32) -> String {
        format!("nightly-{n}-0000000{}", n % 10)
    }

    /// A chain to release `target` from the kept file of `base`, over releases `all` (by number).
    fn run(all: &BTreeMap<u32, (Manifest, Files)>, target: u32, base: u32) -> (Option<Vec<String>>, usize) {
        let (m, f) = &all[&target];
        let reads = std::cell::Cell::new(0);
        let mut earlier = |v: &str| {
            reads.set(reads.get() + 1);
            Ok(all.values().find(|(m, _)| m.version == v).cloned())
        };
        let kept = |_: &str, s: &str| s == sha(base);
        let c = find(m, f, &kept, &mut earlier, &mut |_| {});
        (c.map(|c| c.iter().map(|h| h.delta.file.name.clone()).collect()), reads.get())
    }

    /// Releases 1..=n, each with deltas from the 5 before it.
    fn line(n: u32, size: u64) -> BTreeMap<u32, (Manifest, Files)> {
        (1..=n).map(|i| (i, rel(i, &(i.saturating_sub(5).max(1)..i).collect::<Vec<_>>(), size))).collect()
    }

    #[test]
    fn two_hops_then_five_and_never_six() {
        let all = line(30, 10);
        // (one delta: no chain needed, but find finds it too)
        assert_eq!(run(&all, 10, 9).0, Some(vec!["d9-10".into()]));
        // (2 hops: 10 from 5... a base 6 back from 11)
        assert_eq!(run(&all, 11, 5).0.map(|c| c.len()), Some(2));
        // (5 hops: 25 from base 0? the first release 1 is 24 back: 5 hops reach 25 back)
        let c = run(&all, 26, 1).0.unwrap();
        assert_eq!(c.len(), 5, "{c:?}");
        // (6 hops refused: base 1, target 27 is 26 back)
        assert_eq!(run(&all, 27, 1).0, None);
        // (the fewest hops win: base 20 to 25 is one hop)
        assert_eq!(run(&all, 25, 20).0, Some(vec!["d20-25".into()]));
    }

    #[test]
    fn a_chain_not_smaller_than_the_full_file_is_refused() {
        let all = line(12, 600);
        assert_eq!(run(&all, 7, 2).0.map(|c| c.len()), Some(1), "one delta of 600 < 1000");
        assert_eq!(run(&all, 8, 2).0, None, "8 from 2 needs 2 hops: 1200 is not smaller than the full file");
    }

    #[test]
    fn a_broken_middle_hop_or_a_bad_manifest_is_left_out() {
        let mut all = line(12, 10);
        // (release 6's delta file is not in its files: no hop through it)
        all.get_mut(&11).unwrap().1.clear();
        assert_eq!(run(&all, 11, 1).0, None, "the target's own deltas not listed");
        let mut all = line(12, 10);
        // (release 6's manifest names another full file than its deltas were made from)
        all.get_mut(&6).unwrap().0.full.sha256 = sha(777);
        let c = run(&all, 11, 1).0.unwrap();
        assert!(!c.iter().any(|h| h.ends_with("-6")), "{c:?}");
        // (an earlier manifest that can't be read: said, left out)
        let all = line(12, 10);
        let (m, f) = &all[&11];
        let mut said = Vec::new();
        let mut earlier = |_: &str| Err::<Option<(Manifest, Files)>, String>("bad".into());
        assert!(find(m, f, &|_, s| s == sha(1), &mut earlier, &mut |s| said.push(s)).is_none());
        assert!(said.iter().any(|s| s.contains("no step")), "{said:?}");
    }

    #[test]
    fn the_look_back_window_and_a_nightly_s_stable() {
        let r = |t: &str| Listed { tag: t.into(), draft: false, prerelease: false, assets: vec![] };
        let mut list: Vec<Listed> = (1..=30).map(|i| r(&tag(i))).collect();
        list.push(r("v3.0.0"));
        list.push(r("v3.1.0"));
        let back = look_back(&list, &tag(30));
        assert_eq!(back.len(), LOOK_BACK + 1);
        assert_eq!(back[0], tag(29));
        assert_eq!(back[LOOK_BACK - 1], tag(10));
        assert_eq!(back.last().map(String::as_str), Some("v3.1.0"), "the latest stable, for a nightly");
        assert_eq!(look_back(&list, "v3.1.0"), ["v3.0.0"], "a stable: its own channel alone");
        assert!(look_back(&list, "pr-1-aaaaaaaa").is_empty());
    }

    #[test]
    fn a_stable_into_a_nightly() {
        // (nightly 40 has a delta from v3.1.0, whose file is kept)
        let (mut m, mut f) = rel(40, &[35], 10);
        m.deltas.push(Delta {
            from: "v3.1.0".into(),
            from_sha256: sha(31),
            file: FileEntry { name: "dstable".into(), size: 50, sha256: sha(5) },
        });
        f.insert("dstable".into(), "https://x/dstable".into());
        let mut earlier = |_: &str| Ok(None);
        let c = find(&m, &f, &|from, s| from == "v3.1.0" && s == sha(31), &mut earlier, &mut |_| {}).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].delta.from, "v3.1.0");
        assert_eq!(m.choose(&|_, s| s == sha(31)).map(|d| d.from.as_str()), Some("v3.1.0"));
    }

    #[test]
    fn at_most_the_look_back_s_manifests_are_read() {
        let all = line(60, 1);
        let (_, reads) = run(&all, 60, 999);
        assert!(reads <= LOOK_BACK + 1, "{reads}");
    }
}

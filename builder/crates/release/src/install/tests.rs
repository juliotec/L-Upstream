//! `fetch` against a fake forge: releases made here, signed with the tests' key, served from memory (no network).

use super::*;
use crate::forge::{Net, NetError};
use crate::trust::tests::{sign, test_key};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

const P: &str = "linux-x64";
const API: &str = "https://git.magicbean.dev/api/v1/repos/MagicBeanDev/Longshot";

/// A forge in memory: its releases' JSON (newest first), its files by link; links that fail; every link asked.
#[derive(Default)]
struct FakeForge {
    releases: Vec<Value>,
    files: BTreeMap<String, Vec<u8>>,
    failing: BTreeSet<String>,
    asked: RefCell<Vec<String>>,
}

fn link(tag: &str, name: &str) -> String {
    format!("https://git.magicbean.dev/MagicBeanDev/Longshot/releases/download/{tag}/{name}")
}

impl Net for FakeForge {
    fn text(&self, url: &str, _max: u64) -> Result<String, NetError> {
        self.asked.borrow_mut().push(url.to_string());
        if url == format!("{API}/releases?limit=50&page=1") {
            return Ok(Value::Array(self.releases.clone()).to_string());
        }
        if let Some(tag) = url.strip_prefix(&format!("{API}/releases/tags/")) {
            return self
                .releases
                .iter()
                .find(|r| r["tag_name"] == tag)
                .map(Value::to_string)
                .ok_or_else(|| NetError::new("HTTP 404"));
        }
        Err(NetError::new("HTTP 404"))
    }

    fn download(&self, url: &str, to: &Path, max: u64, progress: &mut dyn FnMut(u64)) -> Result<(), NetError> {
        self.asked.borrow_mut().push(url.to_string());
        if self.failing.contains(url) {
            return Err(NetError::new("HTTP 500"));
        }
        let b = self.files.get(url).ok_or_else(|| NetError::new("HTTP 404"))?;
        if b.len() as u64 > max {
            return Err(NetError::new("larger than asked"));
        }
        std::fs::write(to, b).map_err(|e| NetError::new(e.to_string()))?;
        progress(b.len() as u64);
        Ok(())
    }
}

thread_local! {
    /// The fake deltas' targets, by SHA-256.
    static TARGETS: RefCell<BTreeMap<String, Vec<u8>>> = RefCell::default();
}

/// A fake delta: `FAKE`, the base's SHA-256 and the target's (its bytes kept aside, for the tests' patcher).
fn fake_delta(base: &[u8], target: &[u8]) -> Vec<u8> {
    let t = crate::sha256_hex(target);
    TARGETS.with(|m| m.borrow_mut().insert(t.clone(), target.to_vec()));
    [b"FAKE".as_slice(), crate::sha256_hex(base).as_bytes(), t.as_bytes()].concat()
}

fn fake_patch(base: Vec<u8>, delta: &Path, out: &Path, _: &Cancel) -> Result<(), String> {
    let d = std::fs::read(delta).map_err(|e| e.to_string())?;
    if !d.starts_with(b"FAKE") || d.len() < 132 {
        return Err("not a fake delta".into());
    }
    if d[4..68] != *crate::sha256_hex(&base).as_bytes() {
        return Err("another base".into());
    }
    let t = String::from_utf8_lossy(&d[68..132]).into_owned();
    let target = TARGETS.with(|m| m.borrow().get(&t).cloned()).ok_or("an unknown target")?;
    std::fs::write(out, target).map_err(|e| e.to_string())
}

fn real_patch(base: Vec<u8>, delta: &Path, out: &Path, c: &Cancel) -> Result<(), String> {
    crate::patch::apply(base, delta, out, c)
}

impl FakeForge {
    /// A release of longshot-builder's kind: its full file `full`, a delta from each `(tag, its full file, delta)`,
    /// its manifest and its signed SHA256SUMS; pushed as the newest.
    fn release(&mut self, tag: &str, full: &[u8], deltas: &[(&str, &[u8], Vec<u8>)]) {
        let full_name = format!("longshot-builder-{tag}-{P}");
        let mut files: Vec<(String, Vec<u8>)> = vec![(full_name.clone(), full.to_vec())];
        let mut ds = Vec::new();
        for (from, from_full, delta) in deltas {
            let name = format!("longshot-builder-{from}-to-{tag}-{P}.zst-patch");
            ds.push(json!({"from": from, "from_sha256": crate::sha256_hex(from_full), "name": name,
                           "size": delta.len(), "sha256": crate::sha256_hex(delta), "long": 30}));
            files.push((name, delta.clone()));
        }
        let channel = Channel::of(tag).unwrap().name();
        let manifest = json!({"v": 1, "version": tag, "channel": channel, "platform": P,
            "full": {"name": full_name, "size": full.len(), "sha256": crate::sha256_hex(full)},
            "deltas": ds, "stamps": {"compiler": "c1", "translator": "t1"}});
        files.push((manifest_name(tag, P), manifest.to_string().into_bytes()));
        self.publish(tag, files);
    }

    /// A release of these files, signed, pushed as the newest.
    fn publish(&mut self, tag: &str, mut files: Vec<(String, Vec<u8>)>) {
        let sums: String = files.iter().map(|(n, b)| format!("{}  {n}\n", crate::sha256_hex(b))).collect();
        let (sk, _) = test_key();
        let sig = sign(&sk, sums.as_bytes());
        files.push(("SHA256SUMS".into(), sums.into_bytes()));
        files.push(("SHA256SUMS.sig".into(), sig.into_bytes()));
        let assets: Vec<Value> = files
            .iter()
            .map(|(n, b)| json!({"name": n, "browser_download_url": link(tag, n), "size": b.len()}))
            .collect();
        for (n, b) in files {
            self.files.insert(link(tag, &n), b);
        }
        self.releases.insert(0, json!({"tag_name": tag, "draft": false, "prerelease": false, "assets": assets}));
    }
}

/// A version folder with release `tag`'s program `bytes` and its record.
fn installed(dir: &Path, tag: &str, bytes: &[u8]) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("Longshot.x86_64"), bytes).unwrap();
    let rec = json!({"v": 1, "version": tag, "channel": "nightly", "platform": P,
        "full": {"name": "x", "size": bytes.len(), "sha256": crate::sha256_hex(bytes)}, "program": "Longshot.x86_64"});
    std::fs::write(dir.join(RECORD), rec.to_string()).unwrap();
    dir.to_path_buf()
}

struct Run {
    result: Result<Fetched, FetchError>,
    said: Vec<String>,
    steps: Vec<Progress>,
}

fn run(forge: &FakeForge, want: &str, out: &Path, from: &[PathBuf], patcher: &Patcher) -> Run {
    let cancel = Cancel::default();
    let (_, key) = test_key();
    let f = Forge::new(forge, vec![key], &cancel, &out.join(".fetch-scratch"));
    let req = Request { want: Want::parse(want), out: out.to_path_buf(), from: from.to_vec(), platform: P.into() };
    let (mut said, mut steps) = (Vec::new(), Vec::new());
    let result =
        fetch(&f, &req, patcher, &cancel, &mut Tell { progress: &mut |p| steps.push(p), say: &mut |s| said.push(s) });
    let _ = std::fs::remove_dir_all(out.join(".fetch-scratch"));
    Run { result, said, steps }
}

fn nightly(n: u32) -> String {
    format!("nightly-{n}-{n:08x}")
}

/// Release n's full file (1000 bytes, its own).
fn full(n: u32) -> Vec<u8> {
    (0..1000u32).map(|i| (i.wrapping_mul(31).wrapping_add(n * 7)) as u8).collect()
}

/// Nightlies 1..=n of the fake kind, each with fake deltas of `size` from the 5 before it.
fn line(n: u32, pad: usize) -> FakeForge {
    let mut f = FakeForge::default();
    for i in 1..=n {
        let ds: Vec<(String, Vec<u8>, Vec<u8>)> = (i.saturating_sub(5).max(1)..i)
            .map(|j| {
                let mut d = fake_delta(&full(j), &full(i));
                d.resize(d.len() + pad, 0);
                (nightly(j), full(j), d)
            })
            .collect();
        let refs: Vec<(&str, &[u8], Vec<u8>)> =
            ds.iter().map(|(t, b, d)| (t.as_str(), b.as_slice(), d.clone())).collect();
        f.release(&nightly(i), &full(i), &refs);
    }
    f
}

fn scratch(name: &str) -> PathBuf {
    crate::tests::scratch(&format!("install-{name}"))
}

fn ok(r: &Run) -> &Fetched {
    match &r.result {
        Ok(f) => f,
        Err(e) => panic!("{e:?}; said {:?}", r.said),
    }
}

#[test]
fn a_real_single_delta_and_a_real_two_hop_chain() {
    let d = scratch("real");
    let (a, b) = crate::patch::tests::fixture_pair();
    let mut c = b.clone();
    for i in (7..c.len()).step_by(7919) {
        c[i] ^= 0x33;
    }
    c.extend_from_slice(b"and a third release");
    let ab = crate::patch::tests::FIXTURE.to_vec();
    let bc = include_bytes!("../../tests/fixtures/delta2.zst-patch").to_vec();
    let mut f = FakeForge::default();
    f.release(&nightly(1), &a, &[]);
    f.release(&nightly(2), &b, &[(&nightly(1), &a, ab)]);
    f.release(&nightly(3), &c, &[(&nightly(2), &b, bc)]);
    let have = installed(&d.join("v1"), &nightly(1), &a);
    // (one delta: nightly 2 from 1)
    let r = run(&f, &nightly(2), &d.join("v2"), std::slice::from_ref(&have), &real_patch);
    let got = ok(&r);
    assert_eq!((got.how, got.from.as_deref()), (How::Delta, Some(nightly(1).as_str())));
    assert!(std::fs::read(d.join("v2/Longshot.x86_64")).unwrap() == b);
    assert_eq!(Record::read(&d.join("v2")).unwrap().version, nightly(2));
    assert!(!d.join("v2").join(STAGING).exists(), "the intermediates cleaned up");
    // (two hops: nightly 3 from 1, through 2)
    let r = run(&f, "nightly", &d.join("v3"), &[have], &real_patch);
    let got = ok(&r);
    assert_eq!((got.how, got.through.clone()), (How::Chain, vec![nightly(2)]));
    assert!(std::fs::read(d.join("v3/Longshot.x86_64")).unwrap() == c);
    assert!(
        f.asked.borrow().iter().all(|u| !u.ends_with(&format!("longshot-builder-{}-{P}", nightly(3)))),
        "no full file"
    );
    // (again: present, nothing downloaded)
    let r = run(&f, &nightly(3), &d.join("v3"), &[], &real_patch);
    assert_eq!(ok(&r).how, How::Present);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn five_hops_and_never_six() {
    let d = scratch("hops");
    let f = line(27, 0);
    let have = installed(&d.join("v1"), &nightly(1), &full(1));
    let r = run(&f, &nightly(26), &d.join("v26"), std::slice::from_ref(&have), &fake_patch);
    assert_eq!((ok(&r).how, ok(&r).through.len()), (How::Chain, 4), "5 hops: 4 releases passed through");
    assert!(r.steps.iter().any(|s| s.step == "delta" && s.hop == 5 && s.hops == 5));
    let r = run(&f, &nightly(27), &d.join("v27"), &[have], &fake_patch);
    assert_eq!(ok(&r).how, How::Full, "6 hops refused: the full file");
    assert!(std::fs::read(d.join("v27/Longshot.x86_64")).unwrap() == full(27));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_chain_bigger_than_the_full_file_is_refused() {
    let d = scratch("big");
    let f = line(8, 400);
    let have = installed(&d.join("v1"), &nightly(1), &full(1));
    // (one hop of 532 bytes < 1000: taken; two (1064) not)
    let r = run(&f, &nightly(6), &d.join("v6"), std::slice::from_ref(&have), &fake_patch);
    assert_eq!(ok(&r).how, How::Delta);
    let r = run(&f, &nightly(7), &d.join("v7"), &[have], &fake_patch);
    assert_eq!(ok(&r).how, How::Full);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_broken_middle_hop_falls_back_to_the_full_file() {
    let d = scratch("broken");
    let mut f = line(11, 0);
    // (the middle delta's download fails)
    let mid = link(&nightly(6), &format!("longshot-builder-{}-to-{}-{P}.zst-patch", nightly(1), nightly(6)));
    f.failing.insert(mid);
    let have = installed(&d.join("v1"), &nightly(1), &full(1));
    let r = run(&f, &nightly(11), &d.join("v11"), &[have], &fake_patch);
    assert_eq!(ok(&r).how, How::Full);
    assert!(r.said.iter().any(|s| s.contains("did not apply")), "{:?}", r.said);
    assert!(std::fs::read(d.join("v11/Longshot.x86_64")).unwrap() == full(11));
    assert!(!d.join("v11").join(STAGING).exists());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_bad_middle_manifest_is_no_step() {
    let d = scratch("badmanifest");
    let mut f = line(11, 0);
    // (nightly 6's manifest is not the one its SHA256SUMS signed)
    let m = link(&nightly(6), &manifest_name(&nightly(6), P));
    f.files.insert(m, b"{}".to_vec());
    let have = installed(&d.join("v1"), &nightly(1), &full(1));
    let r = run(&f, &nightly(11), &d.join("v11"), &[have], &fake_patch);
    let got = ok(&r);
    assert!(!got.through.contains(&nightly(6)), "{:?}", got.through);
    assert!(r.said.iter().any(|s| s.contains("no step")), "{:?}", r.said);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_result_of_another_sha256_falls_back() {
    let d = scratch("hash");
    let f = line(3, 0);
    let have = installed(&d.join("v2"), &nightly(2), &full(2));
    let wrong = |b: Vec<u8>, delta: &Path, out: &Path, c: &Cancel| {
        fake_patch(b, delta, out, c)?;
        std::fs::write(out, b"not it").map_err(|e| e.to_string())
    };
    let r = run(&f, &nightly(3), &d.join("v3"), &[have], &wrong);
    assert_eq!(ok(&r).how, How::Full);
    assert!(r.said.iter().any(|s| s.contains("not the file the release names")), "{:?}", r.said);
    // (a kept program changed since its record: no base, the full file)
    let have = installed(&d.join("v2b"), &nightly(2), &full(2));
    std::fs::write(have.join("Longshot.x86_64"), b"changed").unwrap();
    let r = run(&f, &nightly(3), &d.join("v3b"), &[have], &fake_patch);
    assert_eq!(ok(&r).how, How::Full);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn the_look_back_window_bounds_the_chain() {
    let d = scratch("lookback");
    // (deltas from the release before only: 1 to 3 is 2 hops, but nightly 1 is past a look-back of one)
    let mut f = FakeForge::default();
    f.release(&nightly(1), &full(1), &[]);
    f.release(&nightly(2), &full(2), &[(&nightly(1), &full(1), fake_delta(&full(1), &full(2)))]);
    f.release(&nightly(3), &full(3), &[(&nightly(2), &full(2), fake_delta(&full(2), &full(3)))]);
    let have = installed(&d.join("v1"), &nightly(1), &full(1));
    let r = run(&f, &nightly(3), &d.join("v3"), std::slice::from_ref(&have), &fake_patch);
    assert_eq!(ok(&r).how, How::Chain);
    // (21 releases in between: nightly 2's manifest is past the look-back of 20, never read)
    let mut f = FakeForge::default();
    f.release(&nightly(1), &full(1), &[]);
    f.release(&nightly(2), &full(2), &[(&nightly(1), &full(1), fake_delta(&full(1), &full(2)))]);
    for i in 3..=23 {
        f.release(&nightly(i), &full(i), &[]);
    }
    f.release(&nightly(24), &full(24), &[(&nightly(2), &full(2), fake_delta(&full(2), &full(24)))]);
    let r = run(&f, &nightly(24), &d.join("v24"), &[have], &fake_patch);
    assert_eq!(ok(&r).how, How::Full);
    let asked = f.asked.borrow();
    assert!(!asked.iter().any(|u| u.ends_with(&manifest_name(&nightly(2), P))), "past the look-back: not read");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_stable_into_a_nightly() {
    let d = scratch("stable");
    let mut f = FakeForge::default();
    f.release("v3.0.0", &full(100), &[]);
    f.release(&nightly(5), &full(5), &[("v3.0.0", &full(100), fake_delta(&full(100), &full(5)))]);
    let have = installed(&d.join("stable"), "v3.0.0", &full(100));
    let r = run(&f, "nightly", &d.join("n5"), &[have], &fake_patch);
    let got = ok(&r);
    assert_eq!((got.how, got.from.as_deref()), (How::Delta, Some("v3.0.0")));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_make_game_release_is_refused_clearly() {
    let d = scratch("makegame");
    let mut f = FakeForge::default();
    f.publish(&nightly(9), vec![("Longshot-nightly-9-x86_64.AppImage".into(), b"appimage".to_vec())]);
    let r = run(&f, "nightly", &d.join("v"), &[], &fake_patch);
    let e = r.result.unwrap_err();
    assert_eq!(e.code, "no_builder_in_release");
    assert!(e.text.contains("make_game.py"), "{e:?}");
    // (unsigned: refused; no such channel; no such tag)
    let mut g = FakeForge::default();
    g.release(&nightly(1), &full(1), &[]);
    g.files.insert(link(&nightly(1), "SHA256SUMS.sig"), b"AAAA".to_vec());
    assert_eq!(run(&g, &nightly(1), &d.join("w"), &[], &fake_patch).result.unwrap_err().code, "release_refused");
    assert_eq!(run(&f, "stable", &d.join("w"), &[], &fake_patch).result.unwrap_err().code, "no_release_on_channel");
    assert_eq!(run(&f, "v3.9.9", &d.join("w"), &[], &fake_patch).result.unwrap_err().code, "no_such_release");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_kept_program_with_no_record_is_hashed_and_a_windowed_one_read_as_released() {
    let d = scratch("norecord");
    let console = crate::swap::tests::fake_pe(3);
    let mut next = console.clone();
    next.extend_from_slice(b"newer");
    let mut f = FakeForge::default();
    f.release(&nightly(1), &console, &[]);
    f.release(&nightly(2), &next, &[(&nightly(1), &console, fake_delta(&console, &next))]);
    // (a standalone copy: its Longshot.x86_64 windowed, no record)
    let dir = d.join("old");
    std::fs::create_dir_all(&dir).unwrap();
    let mut windowed = console.clone();
    crate::swap::set_subsystem(&mut windowed, 2).unwrap();
    std::fs::write(dir.join("Longshot.x86_64"), &windowed).unwrap();
    let b = bases(std::slice::from_ref(&dir), P);
    assert_eq!(b[0].sha256, crate::sha256_hex(&console), "the full file it was made from");
    let r = run(&f, &nightly(2), &d.join("new"), &[dir], &fake_patch);
    assert_eq!(ok(&r).how, How::Delta);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_version_the_launcher_installed_is_a_base_by_its_toolchain_json() {
    let d = scratch("toolchain");
    let mut f = FakeForge::default();
    f.release(&nightly(1), &full(1), &[]);
    f.release(&nightly(2), &full(2), &[(&nightly(1), &full(1), fake_delta(&full(1), &full(2)))]);
    let old = d.join(nightly(1));
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("longshot-builder"), full(1)).unwrap();
    let rec = json!({"format": 1, "tag": nightly(1), "kind": "longshotBuild", "run": "longshot-builder",
                     "sha256": crate::sha256_hex(&full(1)), "size": 1000});
    std::fs::write(old.join("toolchain.json"), rec.to_string()).unwrap();
    let r = run(&f, &nightly(2), &d.join("new"), &[old], &fake_patch);
    assert_eq!(ok(&r).how, How::Delta);
    std::fs::remove_dir_all(&d).unwrap();
}

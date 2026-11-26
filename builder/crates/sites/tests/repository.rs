//! The repository's own tables (`game`): every build's locations read with its definitions, every site with its
//! line in the host's list, and every engine site placed by a table. Game-free: the tables hold names, offsets and a
//! few masked words per site, nothing else of the game.

use sites::{HostList, ResolveError, Space, Tables, ident, is_name, load, parse_definitions, resolve};
use std::path::PathBuf;

/// The build the repository supports (the one table it must have).
const SUPPORTED: &str = "e0f3ccfaf50065ae4ea01eb5aeb13b1a45da3348";

fn game() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../game"))
}

fn host_text() -> String {
    std::fs::read_to_string(game().join("sites.def")).unwrap()
}

fn builds() -> Vec<String> {
    // (a build's folder each; sites.toml, the definitions, is beside them)
    let mut b: Vec<String> = std::fs::read_dir(game().join("sites"))
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    b.sort();
    b
}

#[test]
fn every_table_reads_and_has_its_host_lines() {
    let tables = Tables::new(game());
    let host = tables.host().unwrap().expect("game/sites.def");
    let builds = builds();
    assert!(builds.iter().any(|b| b == SUPPORTED), "no table for the supported build");
    let defs_path = tables.definitions();
    let defs = parse_definitions(&std::fs::read_to_string(&defs_path).unwrap(), "game/sites/sites.toml").unwrap();
    let mut placed = std::collections::BTreeSet::new();
    for b in &builds {
        assert!(b.len() == 40 && b.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "{b}: not a build id");
        let sites = load(&tables, b, Some(&host)).unwrap().unwrap_or_else(|| panic!("{b} has no locations.toml"));
        for s in &sites {
            placed.insert(s.name.clone());
            match s.code().and_then(|c| c.rule.as_ref()) {
                // (an inline rule has no flag or slot: the value it reads has its line)
                Some(r) => assert!(host.values.contains_key(&r.value), "{}: no HOS_VALUE line for {}", s.name, r.value),
                None => assert_eq!(host.sites.get(&s.ident()), Some(&s.name), "{}: no HOS_SITE line", s.name),
            }
            // (as few words as find it: docs/dev/reference/hooks.md, what the repository may hold)
            if let Some(sig) = s.code().and_then(|c| c.signature.as_ref()) {
                assert!(sig.len() <= 8, "{}: a signature of {} words", s.name, sig.len());
            }
        }
        // (each required definition is located in every build: load would stop otherwise; and only defined ones)
        let mut want: Vec<&str> = defs
            .iter()
            .filter_map(|d| {
                let name = d.get("name").and_then(|v| v.as_str())?;
                let required = d.get("require").and_then(|v| v.as_bool()).unwrap_or(true);
                (required || sites.iter().any(|s| s.name == name)).then_some(name)
            })
            .collect();
        let mut got: Vec<&str> = sites.iter().map(|s| s.name.as_str()).collect();
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, want, "{b}");
    }
    // (every engine callback's site is a row of a table, or nothing places its hook and the feature is silently gone)
    assert!(!host.engine.is_empty(), "game/sites.def has no HOS_SITE_ENGINE line: this check reads it wrong");
    let unplaced: Vec<&String> = host.engine.iter().filter(|n| !placed.contains(*n)).collect();
    assert!(unplaced.is_empty(), "HOS_SITE_ENGINE lines whose site no table has: {unplaced:?}");
    // (each definition has its host line, or is a rule)
    for d in &defs {
        let name = d.get("name").and_then(|v| v.as_str()).unwrap();
        assert!(host.sites.values().any(|n| n == name) || d.contains_key("rule"), "{name}: no host line");
    }
}

#[test]
fn the_worker_outcome_sites_are_listed() {
    // (game/sites.def's HOS_SITE_OUTCOME lines are exactly the sites that change the game's outcome on a worker
    // thread, so the host refuses them a run-time handler; the lift stops on one missing or one too many)
    let tables = Tables::new(game());
    let host = tables.host().unwrap().expect("game/sites.def");
    for b in builds() {
        let sites = load(&tables, &b, Some(&host)).unwrap().unwrap();
        let want: std::collections::BTreeMap<String, String> =
            sites.iter().filter(|s| s.decides_on_worker()).map(|s| (s.ident(), s.name.clone())).collect();
        assert!(!want.is_empty(), "no worker-thread outcome site in {b}: this check reads the table wrong");
        assert_eq!(host.outcome, want, "game/sites.def's HOS_SITE_OUTCOME lines are not {b}'s outcome sites");
    }
    let starts = host_text().lines().filter(|l| l.starts_with("HOS_SITE_OUTCOME(")).count();
    assert_eq!(starts, host.outcome.len(), "a HOS_SITE_OUTCOME line the reader does not take, or one named twice");
}

#[test]
fn the_host_list_names_each_site_once() {
    let text = host_text();
    let lines = HostList::site_lines(&text);
    assert!(!lines.is_empty());
    let idents: std::collections::BTreeSet<&String> = lines.iter().map(|(i, _)| i).collect();
    assert_eq!(idents.len(), lines.len(), "an identifier named twice in game/sites.def");
    for (i, n) in &lines {
        assert!(is_name(n), "{n}: not a site name");
        assert_eq!(&ident(n), i, "{n}: its identifier is not {i}");
    }
    // (and lines that start like a site's but are none would be left out: every HOS_SITE line is one)
    let starts = text.lines().filter(|l| l.starts_with("HOS_SITE(") || l.starts_with("HOS_SITE_ENGINE(")).count();
    assert_eq!(starts, lines.len(), "a HOS_SITE line the reader does not take");
}

// ---- the drive target's two sites (tests/test_drive_target.py checks their callbacks)

const DRIVE_ENTRY: u64 = 0x804f_5030; // (f_804f5030)

fn ldr_x(rt: u32, rn: u32, imm: u32) -> u32 {
    0xf940_0000 | (imm / 8) << 10 | rn << 5 | rt
}
fn ldp_s(rt: u32, rt2: u32, rn: u32, imm: u32) -> u32 {
    0x2d40_0000 | (imm / 4) << 15 | rt2 << 10 | rn << 5 | rt
}
fn ldr_s(rt: u32, rn: u32, imm: u32) -> u32 {
    0xbd40_0000 | (imm / 4) << 10 | rn << 5 | rt
}

/// One copy of the path as tests/test_drive_target.py's READ models it: the body's load from the controller +0xf0 into
/// `reg`, then the target's reads at +0x3c and +0x44.
fn copy(reg: u32) -> Vec<u32> {
    vec![ldr_x(reg, 20, 0xf0), ldp_s(2, 1, 20, 0x3c), ldr_s(0, 20, 0x44)]
}

/// Made-up code from f_804f5030: nops, with `copies` (offset, words) written in.
fn made_up(copies: &[(u64, Vec<u32>)]) -> Vec<u32> {
    let mut words = vec![0xd503_201f; 0x1000 / 4];
    for (off, ws) in copies {
        let at = (*off / 4) as usize;
        words[at..at + ws.len()].copy_from_slice(ws);
    }
    words
}

fn resolve_in(sites: &[sites::Site], code: Vec<u32>) -> Result<sites::Found, ResolveError> {
    let space = Space { base: DRIVE_ENTRY, code, image: &[], image_base: DRIVE_ENTRY, exports: Default::default() };
    resolve(sites, &space)
}

#[test]
fn the_drive_target_sites_are_each_found_once_at_their_copy() {
    // (each site is its copy's load of the body into the register its callback hands on, then the target's reads, in
    // whole words, its hook after the load, found by its signature alone; in made-up code with the two copies at their
    // offsets the resolver finds each exactly once, and stops naming the site when its copy changes or a third appears)
    let tables = Tables::new(game());
    let host = tables.host().unwrap().unwrap();
    let all = load(&tables, SUPPORTED, Some(&host)).unwrap().unwrap();
    let by = |n: &str| all.iter().find(|s| s.name == n).cloned().unwrap_or_else(|| panic!("no site {n}"));
    let sites = [by("physics.driveTarget"), by("physics.driveTargetOther")];
    let mut copies = Vec::new();
    for (s, reg) in sites.iter().zip([23, 24]) {
        let c = s.code().unwrap();
        let sig: Vec<(u32, u32)> =
            c.signature.as_ref().expect("a signature").words().iter().map(|w| (w.value, w.mask)).collect();
        let want: Vec<(u32, u32)> = copy(reg).into_iter().map(|w| (w, u32::MAX)).collect();
        assert_eq!(sig, want, "{} is not ldr x{reg}, [x20, #0xf0] before the target's reads, in whole words", s.name);
        let offset = c.offset.unwrap();
        assert_eq!(c.signature_offset, offset as i64, "{}: its hook is not at its signature", s.name);
        assert!(
            c.point.is_none() && c.fingerprint.is_none() && c.guards.is_empty() && c.inside.is_none(),
            "{} is found by more than its signature (jump.takeoffVelocity is found from physics.driveTargetOther's \
             place, which the resolver gives without the lift)",
            s.name
        );
        assert_eq!(c.function, DRIVE_ENTRY);
        copies.push((offset, copy(reg)));
    }
    let velocity = by("jump.takeoffVelocity");
    let v = velocity.code().unwrap();
    assert!(
        v.inside.as_deref() == Some("physics.driveTargetOther")
            && v.point.as_ref().is_some_and(|p| p.start == Some(sites::End::Site("physics.driveTargetOther".into()))),
        "jump.takeoffVelocity is no longer found from physics.driveTargetOther: the reason for keeping that site's \
         signature alone is gone"
    );
    let got = resolve_in(&sites, made_up(&copies)).unwrap();
    for (s, (off, _)) in sites.iter().zip(&copies) {
        assert_eq!(got[&s.name].addr, DRIVE_ENTRY + off, "{} in made-up code", s.name);
    }
    let fails = |code: Vec<u32>, name: &str, why: &str| {
        let e = resolve_in(&sites, code).unwrap_err().to_string();
        assert!(e.contains(&format!("  {name}: {why}")), "the resolver's stop does not say {name}: {why}:\n{e}");
    };
    let mut moved = copies.clone();
    moved[0].1[0] = ldr_x(23, 20, 0xf8); // (the body at another offset of the controller)
    fails(made_up(&moved), &sites[0].name, "not found");
    let mut third = copies.clone();
    third.push((0x800, copy(23))); // (the first copy written again elsewhere)
    fails(made_up(&third), &sites[0].name, "found 2 times");
}

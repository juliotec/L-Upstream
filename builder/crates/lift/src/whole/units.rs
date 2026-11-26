//! The lifted functions' units: which functions go in which `.ll` file, and the files written, in parallel.
//!
//! **Cut by cost, at stable places.** Functions go into units in address order, the profile's hot ones into units of
//! their own (`lifted_hot_*`, built at the hot code's level), and the ones it saw never run into units of their own too
//! (`lifted_cold_*`, built at the cold code's level, cheaply: [`Sizing::cold_budget`]). A unit holds about [`Sizing::budget`] of estimated compile
//! cost ([`cost`]: a function's instructions, plus their square over [`QUAD`], as LLVM's passes over one big function
//! grow faster than its size), so no unit compiles alone for minutes at the end of a build. Where a unit ends is
//! decided by the functions around it, not by a running count from the start: past half its budget, a unit ends
//! before a function whose entry's hash ends in four zero bits (about one in sixteen), and before any function that
//! would take it past its budget. A change in one function so moves at most the boundaries next to it, and every
//! other unit keeps its functions, its name (`lifted_<first entry>.ll`) and its text.
//!
//! **Keyed.** Each unit has a key made before it is lifted ([`Unit::key`]): the lift's base key (the lifter, the
//! options, whether the units are instrumented) and, for each of its functions, its entry, its edits and its profile
//! counts. A unit whose key is the one `units.txt` gives for it, and whose file is there with its size, is not lifted
//! again.

use build_core::key::Key;
use std::collections::{BTreeMap, BTreeSet};

/// A function's size in instructions past which its cost is twice its size (an estimate, from compile times measured
/// at about 4,100 instructions; IR is costed the same way in bytes in [`compile_cost`]).
pub const QUAD: u64 = 4096;

/// A function's estimated compile cost, from its instruction count.
pub fn cost(insns: u64) -> u64 {
    insns + insns * insns / QUAD
}

/// The IR bytes past which a function's compile cost is twice its size (`QUAD` instructions of about 340 bytes of IR
/// each, as the game's measure).
pub const QUAD_IR: u64 = QUAD * IR_PER_COST;

/// A unit's compile cost from its functions' IR sizes in bytes, for the compile's order (`units.txt`).
pub fn compile_cost(sizes: &[u64]) -> u64 {
    sizes.iter().map(|&b| b + b * b / QUAD_IR).sum()
}

/// How big units are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sizing {
    /// A regular unit's budget, in [`cost`] units.
    pub budget: u64,
    /// A hot unit's (built at a higher level: smaller).
    pub hot_budget: u64,
    /// A cold unit's (built at a lower level: larger); 0: the functions that never ran stay in the regular units.
    pub cold_budget: u64,
}

impl Default for Sizing {
    /// About 25 MB of IR a regular unit, a quarter of that a hot one, three times that a cold one (-O0 compiles the
    /// game's lifted IR in a fifth of -O1's time).
    fn default() -> Sizing {
        Sizing { budget: 80_000, hot_budget: 20_000, cold_budget: 240_000 }
    }
}

/// One unit, planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Its file's name without `.ll`: `lifted_<first entry>`, `lifted_hot_<first entry>` or `lifted_cold_<first entry>`.
    pub name: String,
    /// Its functions' entries, ascending.
    pub functions: Vec<u64>,
    /// Their estimated cost.
    pub cost: u64,
    /// Its key, made before it is lifted.
    pub key: String,
}

/// A 64-bit mix of an entry (splitmix64's finaliser): where units may end.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// One stream of functions cut into units.
fn cut(prefix: &str, functions: &[(u64, u64)], budget: u64) -> Vec<(String, Vec<u64>, u64)> {
    let mut out: Vec<(String, Vec<u64>, u64)> = Vec::new();
    let (mut cur, mut sum) = (Vec::new(), 0u64);
    for &(entry, c) in functions {
        let ends = !cur.is_empty() && (sum + c > budget || (sum >= budget / 2 && mix(entry) & 15 == 0));
        if ends {
            let first: u64 = cur[0];
            out.push((format!("{prefix}_{first:08x}"), std::mem::take(&mut cur), sum));
            sum = 0;
        }
        cur.push(entry);
        sum += c;
    }
    if let Some(&first) = cur.first() {
        out.push((format!("{prefix}_{first:08x}"), cur, sum));
    }
    out
}

/// The units of these functions (entry and instruction count, ascending), `hot` ones apart and `cold` ones apart (when
/// [`Sizing::cold_budget`] is not 0; a function in both is hot), each keyed from `base` and its functions' keys
/// (`key_of`: a function's inputs, its edits and its profile counts).
pub fn plan(
    functions: &[(u64, u64)],
    hot: &BTreeSet<u64>,
    cold: &BTreeSet<u64>,
    sizing: Sizing,
    base: &str,
    key_of: &dyn Fn(u64) -> String,
) -> Vec<Unit> {
    let (mut regular, mut hots, mut colds) = (Vec::new(), Vec::new(), Vec::new());
    for &(e, n) in functions {
        let list = if hot.contains(&e) {
            &mut hots
        } else if sizing.cold_budget > 0 && cold.contains(&e) {
            &mut colds
        } else {
            &mut regular
        };
        list.push((e, cost(n)));
    }
    let mut units = Vec::new();
    for (prefix, list, budget) in [
        ("lifted", &regular, sizing.budget),
        ("lifted_hot", &hots, sizing.hot_budget),
        ("lifted_cold", &colds, sizing.cold_budget),
    ] {
        for (name, functions, cost) in cut(prefix, list, budget.max(1)) {
            let mut k = Key::new();
            k.text("base", base).text("unit", &name);
            for &e in &functions {
                k.part("entry", &e.to_le_bytes()).text("function", &key_of(e));
            }
            units.push(Unit { name, functions, cost, key: k.hex() });
        }
    }
    units
}

/// A unit as `units.txt` lists it: its name, compile cost, size and keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    /// Its file's name without `.ll`.
    pub name: String,
    /// Its compile cost ([`compile_cost`]).
    pub cost: u64,
    /// Its file's size in bytes.
    pub bytes: u64,
    /// Its key, made before it was lifted ([`Unit::key`]).
    pub key: String,
    /// Its text's SHA-1: the compile's object key starts from it.
    pub content: String,
    /// How many functions it defines.
    pub functions: usize,
}

/// What a pending unit's content key is listed as: not known yet (its text not made).
pub const PENDING: &str = "-";

/// IR bytes an instruction costs, about (the game's measure): a pending unit's estimated size.
pub const IR_PER_COST: u64 = 340;

impl Listed {
    /// A unit planned whose text is not made yet: its key, its size and cost estimated, its content key [`PENDING`].
    pub fn pending(u: &Unit) -> Listed {
        // (a function of n instructions is about 340 n bytes of IR, whose compile cost is then 340 times its cost)
        let estimate = u.cost * IR_PER_COST;
        Listed {
            name: u.name.clone(),
            cost: estimate,
            bytes: estimate,
            key: u.key.clone(),
            content: PENDING.into(),
            functions: u.functions.len(),
        }
    }

    /// Whether its text has not been made yet.
    pub fn is_pending(&self) -> bool {
        self.content == PENDING
    }
}

/// `units.txt`: a line per unit, `<name>.ll <cost> <bytes> <key> <content sha1> <functions>`, most costly first (the
/// compile's order); a unit whose text is not made yet has `-` for its content key, its cost and size estimated.
pub fn list_text(units: &[Listed]) -> String {
    let mut sorted: Vec<&Listed> = units.iter().collect();
    sorted.sort_by(|a, b| b.cost.cmp(&a.cost).then_with(|| a.name.cmp(&b.name)));
    let mut s =
        String::from("# longshot-builder units 1: <file> <compile cost> <bytes> <key> <content sha1> <functions>\n");
    for u in sorted {
        s.push_str(&format!("{}.ll {} {} {} {} {}\n", u.name, u.cost, u.bytes, u.key, u.content, u.functions));
    }
    s
}

/// The units a `units.txt` lists, by name (an unreadable line is left out: its unit is lifted again).
pub fn parse_list(text: &str) -> BTreeMap<String, Listed> {
    let mut out = BTreeMap::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let f: Vec<&str> = line.split(' ').collect();
        let [file, cost, bytes, key, content, functions] = f[..] else { continue };
        let (Some(name), Ok(cost), Ok(bytes), Ok(functions)) =
            (file.strip_suffix(".ll"), cost.parse(), bytes.parse(), functions.parse())
        else {
            continue;
        };
        let listed = Listed { name: name.into(), cost, bytes, key: key.into(), content: content.into(), functions };
        out.insert(name.to_string(), listed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn funcs(n: u64) -> Vec<(u64, u64)> {
        (0..n).map(|i| (0x8000_0000 + 0x40 * i, 10 + (i * 7) % 50)).collect()
    }

    #[test]
    fn units_stay_within_budget_and_cover_everything() {
        let f = funcs(5000);
        let sizing = Sizing { budget: 2000, hot_budget: 500, cold_budget: 0 };
        let units = plan(&f, &BTreeSet::new(), &BTreeSet::new(), sizing, "b", &|_| String::new());
        let all: Vec<u64> = units.iter().flat_map(|u| u.functions.clone()).collect();
        assert_eq!(all, f.iter().map(|x| x.0).collect::<Vec<_>>());
        assert!(units.iter().all(|u| u.cost <= 2000));
        assert!(units.iter().all(|u| u.name == format!("lifted_{:08x}", u.functions[0])));
    }

    #[test]
    fn a_change_moves_only_nearby_boundaries() {
        let sizing = Sizing { budget: 2000, hot_budget: 500, cold_budget: 0 };
        let none = |_: u64| String::new();
        let a = plan(&funcs(5000), &BTreeSet::new(), &BTreeSet::new(), sizing, "b", &none);
        let mut grown = funcs(5000);
        grown[2500].1 = 900; // (one function much bigger)
        let b = plan(&grown, &BTreeSet::new(), &BTreeSet::new(), sizing, "b", &none);
        let same = a.iter().filter(|u| b.contains(u)).count();
        assert!(same + 4 >= a.len(), "{same} of {} units kept", a.len());
    }

    #[test]
    fn hot_functions_have_units_of_their_own() {
        let f = funcs(100);
        let hot: BTreeSet<u64> = f.iter().step_by(10).map(|x| x.0).collect();
        let units = plan(&f, &hot, &BTreeSet::new(), Sizing::default(), "b", &|_| String::new());
        let hot_units: Vec<&Unit> = units.iter().filter(|u| u.name.starts_with("lifted_hot_")).collect();
        assert_eq!(hot_units.iter().map(|u| u.functions.len()).sum::<usize>(), hot.len());
    }

    #[test]
    fn never_run_functions_have_units_of_their_own_unless_asked_not_to() {
        let f = funcs(100);
        let hot: BTreeSet<u64> = f.iter().step_by(10).map(|x| x.0).collect();
        // (every third one never ran; a function both hot and cold, as no profile says, is hot)
        let cold: BTreeSet<u64> = f.iter().step_by(3).map(|x| x.0).collect();
        let none = |_: u64| String::new();
        let units = plan(&f, &hot, &cold, Sizing::default(), "b", &none);
        let of = |prefix: &str| -> BTreeSet<u64> {
            units.iter().filter(|u| u.name.starts_with(prefix)).flat_map(|u| u.functions.clone()).collect()
        };
        assert_eq!(of("lifted_hot_"), hot);
        assert_eq!(of("lifted_cold_"), cold.difference(&hot).copied().collect());
        assert_eq!(units.iter().map(|u| u.functions.len()).sum::<usize>(), f.len());
        let off = Sizing { cold_budget: 0, ..Sizing::default() };
        assert!(plan(&f, &hot, &cold, off, "b", &none).iter().all(|u| !u.name.starts_with("lifted_cold_")));
    }

    #[test]
    fn the_list_reads_back() {
        let l = Listed {
            name: "lifted_80000000".into(),
            cost: 5,
            bytes: 7,
            key: "k".into(),
            content: "c".into(),
            functions: 2,
        };
        assert_eq!(parse_list(&list_text(std::slice::from_ref(&l))).get("lifted_80000000"), Some(&l));
    }
}

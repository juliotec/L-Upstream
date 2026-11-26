//! Each site found in the game's code exactly once, or the lift stops naming it: a hook is never placed by guess.
//!
//! [`resolve`] needs only the module's code: a signature's place, and a site at an offset in another's function.
//! [`locate`] needs the lift's functions too: a function by another site's (`in`) or by its fingerprint, a point by its
//! anchor (words, a call, a vtable call, a string), each exactly once.

use crate::ResolveError;
use crate::code::{Function, InsnKind, Lift, Space};
use crate::signature::Signature;
use crate::table::{self, Anchor, AnchorKind, End, Fingerprint, MOST_PLACES, Part, Site};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Where a site was found: the address its signature's first word is at (its function's entry, for a site with no
/// signature), how it was found, and the offset of its point when an anchor found it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Location {
    /// The signature's first word, or the function's entry.
    pub addr: u64,
    /// How it was found (`signature`, `in a.b's function`, `fingerprint`, with `; point: ...`).
    pub how: Option<String>,
    /// Bytes from the entry to the point, when an anchor found it.
    pub offset: Option<u64>,
}

/// Each site's location, by name.
pub type Found = BTreeMap<String, Location>;

/// `+0x10` or `-0x4`, as Python's `%#x` writes a signed number.
fn signed_hex(v: i64) -> String {
    if v < 0 { format!("-{:#x}", v.unsigned_abs()) } else { format!("{v:#x}") }
}

fn places(hits: &[u64]) -> String {
    let some: Vec<String> = hits.iter().take(4).map(|a| format!("{a:#x}")).collect();
    format!("found {}{} times ({})", if hits.len() >= 8 { "at least " } else { "" }, hits.len(), some.join(", "))
}

fn entry_of(addr: u64, signature_offset: i64) -> u64 {
    addr.wrapping_add_signed(-signature_offset)
}

/// Whether `sig`'s words are at `addr` of the module's code.
fn sig_at(sig: &Signature, space: &Space<'_>, addr: u64) -> bool {
    space.index(addr).is_some_and(|i| sig.matches_at(&space.code, i))
}

/// The first guard that differs at the function `entry`, by its offset.
fn failed_guard(site: &table::CodeSite, space: &Space<'_>, entry: u64) -> Option<u64> {
    site.guards.iter().find(|g| !sig_at(&g.signature, space, entry + g.offset)).map(|g| g.offset)
}

/// Each code site with a signature found exactly once in the module's code (and its guards where it puts the function),
/// and each site at an offset in such a site's function. The sites found only with the lift are [`locate`]'s.
pub fn resolve(sites: &[Site], space: &Space<'_>) -> Result<Found, ResolveError> {
    let mut out = Found::new();
    let mut bad = Vec::new();
    for s in sites {
        let Some(c) = s.code() else { continue };
        let Some(sig) = &c.signature else { continue };
        let hits = sig.find(&space.code, space.base, 8);
        match hits[..] {
            [hit] => match failed_guard(c, space, entry_of(hit, c.signature_offset)) {
                Some(off) => bad.push(format!(
                    "  {}: found at {hit:#x}, but its guard at +{off:#x} of the function differs",
                    s.name
                )),
                None => {
                    out.insert(s.name.clone(), Location { addr: hit, ..Location::default() });
                }
            },
            [] => {
                let close = sig.near(&space.code, space.base, 4);
                let nearest = if close.is_empty() {
                    String::new()
                } else {
                    let each: Vec<String> = close.iter().map(|(a, j)| format!("{a:#x} (word {j} differs)")).collect();
                    format!("; nearest, all its words but one: {}", each.join(", "))
                };
                bad.push(format!("  {}: not found{nearest}", s.name));
            }
            _ => bad.push(format!("  {}: {}", s.name, places(&hits))),
        }
    }
    let by_name: BTreeMap<&str, &table::CodeSite> =
        sites.iter().filter_map(|s| s.code().map(|c| (s.name.as_str(), c))).collect();
    for s in sites {
        let Some(c) = s.code() else { continue };
        let Some(inside) = &c.inside else { continue };
        if c.needs_lift() {
            continue;
        }
        let Some(other) = by_name.get(inside.as_str()) else { continue };
        if other.inside.is_some() || other.signature.is_none() {
            continue; // (found with the lift, or through a third site: locate()'s)
        }
        let Some(at) = out.get(inside.as_str()) else {
            bad.push(format!("  {}: in {inside}'s function, which does not resolve", s.name));
            continue;
        };
        let entry = entry_of(at.addr, other.signature_offset);
        if let Some(off) = failed_guard(c, space, entry) {
            bad.push(format!(
                "  {}: in {inside}'s function, but its guard at +{off:#x} of the function differs",
                s.name
            ));
            continue;
        }
        out.insert(s.name.clone(), Location { addr: entry, ..Location::default() });
    }
    if bad.is_empty() { Ok(out) } else { Err(ResolveError::Signatures(bad)) }
}

/// Why a site is not found yet: it waits on another site, or it does not resolve.
enum Stop {
    Wait,
    Bad(String),
}

/// The addresses of each NUL-terminated copy of `text` in the module's image, at the start of a string.
pub fn string_addresses(space: &Space<'_>, text: &str) -> BTreeSet<u64> {
    let mut needle = text.as_bytes().to_vec();
    needle.push(0);
    let image = space.image;
    if needle.len() > image.len() {
        return BTreeSet::new();
    }
    (0..=image.len() - needle.len())
        .filter(|&at| (at == 0 || image[at - 1] == 0) && image[at..at + needle.len()] == needle[..])
        .map(|at| space.image_base + at as u64)
        .collect()
}

fn adrp_page(w: u32, pc: u64) -> Option<u64> {
    if w & 0x9f00_0000 != 0x9000_0000 {
        return None;
    }
    let imm = ((w >> 3) & 0x1f_fffc) | ((w >> 29) & 3);
    let imm = ((u64::from(imm) << 43) as i64) >> 43;
    Some((pc & !0xfff).wrapping_add_signed(imm << 12))
}

/// From an adrp at `pc`, the add within the next four instructions that makes one of `addresses` from its register
/// (`next(j)`: the word j instructions after, if there is one).
fn string_add(pc: u64, w: u32, addresses: &BTreeSet<u64>, next: impl Fn(u64) -> Option<u32>) -> Option<u64> {
    let page = adrp_page(w, pc)?;
    let rd = w & 31;
    for j in 1..=4 {
        let w2 = next(j)?;
        if w2 & 0xffc0_0000 == 0x9100_0000
            && (w2 >> 5) & 31 == rd
            && addresses.contains(&(page + u64::from((w2 >> 10) & 0xfff)))
        {
            return Some(pc + 4 * j);
        }
        if w2 & 31 == rd {
            return None; // (the register written again before an add: not this adrp's address)
        }
    }
    None
}

/// The adds (pc) of adrp + add pairs in `words` (one function's, in address order) that make one of `addresses`.
fn string_adds(words: &[(u64, u32)], addresses: &BTreeSet<u64>) -> Vec<u64> {
    if addresses.is_empty() {
        return Vec::new();
    }
    let pages: BTreeSet<u64> = addresses.iter().map(|a| a & !0xfff).collect();
    let word: HashMap<u64, u32> = words.iter().copied().collect();
    words
        .iter()
        .filter(|(pc, w)| adrp_page(*w, *pc).is_some_and(|p| pages.contains(&p)))
        .filter_map(|&(pc, w)| string_add(pc, w, addresses, |j| word.get(&(pc + 4 * j)).copied()))
        .collect()
}

/// The adds in the whole module that make the address of `text`.
fn string_refs(space: &Space<'_>, text: &str) -> Vec<u64> {
    let addresses = string_addresses(space, text);
    if addresses.is_empty() {
        return Vec::new();
    }
    let pages: BTreeSet<u64> = addresses.iter().map(|a| a & !0xfff).collect();
    let code = &space.code;
    (0..code.len())
        .filter_map(|i| {
            let pc = space.base + 4 * i as u64;
            adrp_page(code[i], pc).filter(|p| pages.contains(p))?;
            string_add(pc, code[i], &addresses, |j| code.get(i + j as usize).copied())
        })
        .collect()
}

fn bl_target(w: u32, pc: u64) -> Option<u64> {
    (w & 0xfc00_0000 == 0x9400_0000).then(|| pc.wrapping_add_signed(4 * ((i64::from(w & 0x03ff_ffff) << 38) >> 38)))
}

/// The bls in the whole module whose callee, past PLT stubs, is one of `targets`.
fn callers(space: &Space<'_>, lift: &Lift<'_>, targets: &BTreeSet<u64>) -> Vec<u64> {
    let mut reach = targets.clone();
    reach.extend(lift.plt_stubs().filter(|t| targets.contains(&lift.callee(*t))));
    (0..space.code.len())
        .map(|i| (space.base + 4 * i as u64, space.code[i]))
        .filter(|&(pc, w)| bl_target(w, pc).is_some_and(|t| reach.contains(&t)))
        .map(|(pc, _)| pc)
        .collect()
}

fn words_at(sig: &Signature, f: &Function<'_>, pc: u64) -> bool {
    sig.words().iter().enumerate().all(|(j, m)| f.word(pc + 4 * j as u64).is_some_and(|w| m.matches(w)))
}

fn calls_in(f: &Function<'_>, lift: &Lift<'_>, targets: &BTreeSet<u64>) -> Vec<u64> {
    f.pcs().filter(|&pc| matches!(f.kind(pc), Some(InsnKind::Bl(t)) if targets.contains(&lift.callee(t)))).collect()
}

/// The blr xN with `ldr xN, [xM, #slot]` the nearest write of xN in the four instructions before it.
fn vcalls_in(f: &Function<'_>, slot: u64) -> Vec<u64> {
    let mut out = Vec::new();
    for pc in f.pcs() {
        let Some(InsnKind::Blr(n)) = f.kind(pc) else { continue };
        let n = u32::from(n);
        let want = 0xf940_0000 | (((slot / 8) as u32) << 10) | n;
        for j in 1..=4 {
            let Some(prev) = pc.checked_sub(4 * j).and_then(|p| f.word(p)) else { break };
            // (any other write of xN between the load and the call, a MOV or an ADD, is what it calls: not the slot)
            if prev & 0xffc0_001f == 0xf940_0000 | n || a64::decode(prev).is_some_and(|i| i.writes(n as u8)) {
                if prev & 0xffff_fc1f == want {
                    out.push(pc);
                }
                break;
            }
        }
    }
    out
}

/// The sites being located, and what is known of them so far.
struct Locator<'s> {
    lift: &'s Lift<'s>,
    space: &'s Space<'s>,
    sites: BTreeMap<&'s str, &'s table::CodeSite>,
    entry: BTreeMap<String, u64>,
    point: BTreeMap<String, u64>,
    failed: BTreeMap<String, String>,
}

impl Locator<'_> {
    fn target_of(&self, name: &str) -> Result<BTreeSet<u64>, Stop> {
        if self.sites.contains_key(name) {
            if self.failed.contains_key(name) {
                return Err(Stop::Bad(format!("it names {name}, which does not resolve")));
            }
            return self.entry.get(name).map(|&e| BTreeSet::from([e])).ok_or(Stop::Wait);
        }
        if let Some(a) = table::function_address(name) {
            return Ok(BTreeSet::from([a]));
        }
        self.space
            .exports
            .get(name)
            .map(|&a| BTreeSet::from([a]))
            .ok_or_else(|| Stop::Bad(format!("no site, function (f_<address>) or exported symbol is named {name}")))
    }

    fn point_of(&self, name: &str, e: u64) -> Result<u64, Stop> {
        if self.failed.contains_key(name) {
            return Err(Stop::Bad(format!("it names {name}, which does not resolve")));
        }
        let at = *self.point.get(name).ok_or(Stop::Wait)?;
        match self.entry.get(name) {
            Some(&their) if their != e => {
                Err(Stop::Bad(format!("{name}'s point is in f_{their:08x}, not this site's f_{e:08x}")))
            }
            _ => Ok(at),
        }
    }

    fn matches(&self, a: &Anchor, f: &Function<'_>) -> Result<Vec<u64>, Stop> {
        Ok(match &a.kind {
            AnchorKind::Words { words, at } => {
                f.pcs().filter(|&pc| words_at(words, f, pc)).map(|pc| pc + 4 * *at as u64).collect()
            }
            AnchorKind::Call(t) => calls_in(f, self.lift, &self.target_of(t)?),
            AnchorKind::Vcall(slot) => vcalls_in(f, *slot),
            AnchorKind::String(s) => string_adds(&f.words(), &string_addresses(self.space, s)),
        })
    }

    /// The instruction of anchor `a` in `f`.
    fn point(&self, a: &Anchor, f: &Function<'_>, inner: bool) -> Result<u64, Stop> {
        let e = f.entry;
        let hits = self.matches(a, f)?;
        let mut bounds = [None, None];
        for (i, end) in [&a.start, &a.end].into_iter().enumerate() {
            bounds[i] = match end {
                None => None,
                Some(End::Site(name)) => Some(self.point_of(name, e)?),
                Some(End::Anchor(inner_anchor)) => Some(self.point(inner_anchor, f, true)?),
            };
        }
        let [lo, hi] = bounds;
        let kept: Vec<u64> =
            hits.iter().copied().filter(|&pc| lo.is_none_or(|lo| pc > lo) && hi.is_none_or(|hi| pc < hi)).collect();
        let where_ = |pcs: &[u64]| {
            let some: Vec<String> =
                pcs.iter().take(4).map(|&pc| format!("+{}", signed_hex(pc.wrapping_sub(e) as i64))).collect();
            format!("{}{}", some.join(", "), if pcs.len() > 4 { " ..." } else { "" })
        };
        let what = format!("{}{}", if inner { "an end of its slice, " } else { "its point, " }, a.describe());
        if let Some(m) = a.of.filter(|&m| m != kept.len()) {
            return Err(Stop::Bad(format!(
                "{what}: {} match{} in f_{e:08x}{}, not the {m} it expects",
                kept.len(),
                if kept.len() == 1 { "" } else { "es" },
                if kept.is_empty() { String::new() } else { format!(" ({})", where_(&kept)) }
            )));
        }
        let pc = match a.ordinal {
            Some(n) if n >= kept.len() => {
                return Err(Stop::Bad(format!(
                    "{what}: {} match{} in f_{e:08x}{}, no number {n}",
                    kept.len(),
                    if kept.len() == 1 { "" } else { "es" },
                    if kept.is_empty() { String::new() } else { format!(" ({})", where_(&kept)) }
                )));
            }
            Some(n) => kept[n],
            None if kept.len() == 1 => kept[0],
            None => {
                let count =
                    if kept.is_empty() { "not found".to_string() } else { format!("found {} times", kept.len()) };
                let more = if !kept.is_empty() {
                    format!(" ({}): an ordinal, a slice or more words pick one", where_(&kept))
                } else if !hits.is_empty() {
                    format!(
                        " ({} place{} in the whole function: {})",
                        hits.len(),
                        if hits.len() == 1 { "" } else { "s" },
                        where_(&hits)
                    )
                } else {
                    String::new()
                };
                return Err(Stop::Bad(format!("{what}: {count} in f_{e:08x}{more}")));
            }
        };
        let pc = pc.wrapping_add_signed(4 * a.shift);
        if !f.contains(pc) {
            return Err(Stop::Bad(format!(
                "{what}: shifted {:+} to +{}, not an instruction of f_{e:08x}",
                a.shift,
                signed_hex(pc.wrapping_sub(e) as i64)
            )));
        }
        Ok(pc)
    }

    fn passes(&self, part: &Part, f: &Function<'_>) -> Result<bool, Stop> {
        Ok(match part {
            Part::Call(t) => !calls_in(f, self.lift, &self.target_of(t)?).is_empty(),
            Part::String(s) => !string_adds(&f.words(), &string_addresses(self.space, s)).is_empty(),
            Part::Words(w) => f.pcs().any(|pc| words_at(w, f, pc)),
            Part::Size(lo, hi) => (*lo..=*hi).contains(&(f.len() as u64)),
            Part::Blocks(lo, hi) => {
                let mut blocks = f.labels().clone();
                blocks.insert(f.entry);
                (*lo..=*hi).contains(&(blocks.len() as u64))
            }
        })
    }

    /// The one function of the lift that passes every test of the fingerprint.
    fn fingerprint(&self, fp: &Fingerprint) -> Result<u64, Stop> {
        let parts = fp.parts();
        let mut best: Option<(usize, String, Vec<u64>)> = None;
        for part in &parts {
            let pcs = match part {
                Part::Call(t) => callers(self.space, self.lift, &self.target_of(t)?),
                Part::String(s) => string_refs(self.space, s),
                Part::Words(w) => w.find(&self.space.code, self.space.base, MOST_PLACES + 1),
                Part::Size(..) | Part::Blocks(..) => continue,
            };
            if best.as_ref().is_none_or(|b| pcs.len() < b.0) {
                best = Some((pcs.len(), part.describe(), pcs));
            }
        }
        let Some((count, what, pcs)) = best else {
            return Err(Stop::Bad("no part of the fingerprint finds places in the module".into()));
        };
        if pcs.is_empty() {
            return Err(Stop::Bad(format!("no function {what} (the fingerprint's part found nowhere in the module)")));
        }
        if count > MOST_PLACES {
            return Err(Stop::Bad(format!(
                "its most specific part ({what}) is in more than {MOST_PLACES} places: add a call, a string or words"
            )));
        }
        let candidates: BTreeSet<u64> = pcs.iter().flat_map(|&pc| self.lift.holders(pc)).collect();
        let (mut ok, mut missed) = (Vec::new(), Vec::new());
        for e in candidates {
            let Some(f) = self.lift.function(e) else { continue };
            let mut failed = Vec::new();
            for part in &parts {
                if !self.passes(part, &f)? {
                    failed.push(part.describe());
                }
            }
            if failed.is_empty() { ok.push(e) } else { missed.push((e, failed)) }
        }
        match ok[..] {
            [e] => Ok(e),
            [] => {
                missed.sort_by_key(|(_, f)| f.len());
                let near: Vec<String> =
                    missed.iter().take(4).map(|(e, f)| format!("f_{e:08x} fails {}", f.join(" and "))).collect();
                Err(Stop::Bad(format!("no function passes the fingerprint; nearest: {}", near.join("; "))))
            }
            _ => {
                let some: Vec<String> = ok.iter().take(4).map(|e| format!("f_{e:08x}")).collect();
                Err(Stop::Bad(format!(
                    "the fingerprint matches {} functions ({}): add a test that tells them apart",
                    ok.len(),
                    some.join(", ")
                )))
            }
        }
    }

    /// One site found, or why not (yet).
    fn one(&mut self, s: &Site, c: &table::CodeSite, found: &mut Found) -> Result<(), Stop> {
        let name = s.name.as_str();
        let mut how = found.get(name).and_then(|l| l.how.clone());
        let e = match (self.entry.get(name).copied(), &c.signature) {
            (None, None) => {
                let e = if let Some(inside) = &c.inside {
                    if self.failed.contains_key(inside) {
                        return Err(Stop::Bad(format!("in {inside}'s function, which does not resolve")));
                    }
                    let e = *self.entry.get(inside).ok_or(Stop::Wait)?;
                    how = Some(format!("in {inside}'s function"));
                    e
                } else {
                    let fp = c.fingerprint.as_ref().ok_or_else(|| Stop::Bad("nothing finds its function".into()))?;
                    let e = self.fingerprint(fp)?;
                    how = Some("fingerprint".into());
                    e
                };
                self.entry.insert(name.into(), e);
                e
            }
            (None, Some(_)) => return Err(Stop::Bad("its signature does not resolve".into())),
            (Some(e), _) => {
                let mut h = how.take().unwrap_or_else(|| match &c.inside {
                    None => "signature".into(),
                    Some(inside) => format!("in {inside}'s function"),
                });
                if let (Some(fp), Some(_)) = (&c.fingerprint, &c.signature)
                    && !h.contains("fingerprint")
                {
                    let got = self.fingerprint(fp)?;
                    if got != e {
                        return Err(Stop::Bad(format!("its signature finds f_{e:08x}, its fingerprint f_{got:08x}")));
                    }
                    h += " and fingerprint";
                }
                how = Some(h);
                e
            }
        };
        // (what is known so far stays, for a site that waits and is tried again)
        let at = found
            .entry(name.into())
            .or_insert(Location { addr: e.wrapping_add_signed(c.signature_offset), ..Location::default() });
        at.how.clone_from(&how);
        let Some(f) = self.lift.function(e) else {
            return Err(Stop::Bad(format!("its function would be at {e:#x}, where the lift found none")));
        };
        if c.signature.is_none()
            && let Some(g) = c.guards.iter().find(|g| !words_at(&g.signature, &f, e + g.offset))
        {
            return Err(Stop::Bad(format!("its guard at +{:#x} of f_{e:08x} differs", g.offset)));
        }
        let mut offset = c.offset;
        if let Some(p) = &c.point {
            let pc = self.point(p, &f, false)?;
            offset = Some(pc - e);
            let at = found.get_mut(name).ok_or_else(|| Stop::Bad("lost its place".into()))?;
            at.offset = offset;
            at.how = Some(format!("{}; point: {}", how.unwrap_or_default(), p.describe()));
        }
        self.point.insert(name.into(), e + if c.form.at_offset() { offset.unwrap_or(0) } else { 0 });
        Ok(())
    }
}

/// (after [`resolve`], with the lift's functions) Each site found only with the lift: its function by another site's
/// (`in`) or by its fingerprint (alone, or checking what its signature found), its point by its anchor. `found` gains
/// each site's place and how it was found. A site that does not resolve exactly once stops the lift with every such
/// site named, and why.
pub fn locate(sites: &[Site], lift: &Lift<'_>, found: &mut Found, space: &Space<'_>) -> Result<(), ResolveError> {
    let code: Vec<(&Site, &table::CodeSite)> = sites.iter().filter_map(|s| s.code().map(|c| (s, c))).collect();
    let mut loc = Locator {
        lift,
        space,
        sites: code.iter().map(|(s, c)| (s.name.as_str(), *c)).collect(),
        entry: code
            .iter()
            .filter_map(|(s, c)| found.get(&s.name).map(|l| (s.name.clone(), entry_of(l.addr, c.signature_offset))))
            .collect(),
        point: BTreeMap::new(),
        failed: BTreeMap::new(),
    };
    let mut todo: Vec<(&Site, &table::CodeSite)> = code.clone();
    while !todo.is_empty() {
        let mut later = Vec::new();
        for &(s, c) in &todo {
            match loc.one(s, c, found) {
                Ok(()) => {}
                Err(Stop::Wait) => later.push((s, c)),
                Err(Stop::Bad(why)) => {
                    loc.failed.insert(s.name.clone(), why);
                }
            }
        }
        if later.len() == todo.len() {
            for (s, _) in later {
                loc.failed.insert(
                    s.name.clone(),
                    "it waits on a site that waits on it (in, call, from or to in a cycle)".into(),
                );
            }
            break;
        }
        todo = later;
    }
    if loc.failed.is_empty() {
        return Ok(());
    }
    Err(ResolveError::InTheLift(
        code.iter().filter_map(|(s, _)| loc.failed.get(&s.name).map(|why| format!("  {}: {why}", s.name))).collect(),
    ))
}

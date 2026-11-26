//! The mods' ExeFS patches against the game's hook sites: a patch file that would stop one resolving is refused,
//! naming the site (docs/dev/reference/hooks.md). Here, over game-files' patches and the sites' tables, so neither of
//! those crates depends on the other.

use build_core::{Error, Result};
use game_files::nso::Nso;
use game_files::patches::{Kind, PatchFile};
use std::collections::BTreeMap;

/// A site placed in main's shipped code: the site, its code site, its signature's word index (none for a site in
/// another's function), its function's entry (word index), and the words it needs as they are.
type Placed<'a> = (&'a sites::Site, &'a sites::CodeSite, Option<usize>, i64, std::collections::BTreeSet<i64>);

/// A code or data file that changes the code a hook site is found by or hooked at, or makes its signature match a
/// second place, is refused, its note naming the site: the site is the engine's, and without this the lift would stop.
/// Each file is checked on its own against main as it ships. Returns the refused files' indexes.
pub fn refuse(main: &Nso, tables: &sites::Tables, files: &mut [PatchFile]) -> Result<Vec<usize>> {
    let host = tables.host().map_err(|e| Error::format("hooks", e.to_string()))?;
    let Some(sites) =
        sites::load(tables, &main.build_id, host.as_ref()).map_err(|e| Error::format("hooks", e.to_string()))?
    else {
        return Ok(Vec::new());
    };
    let text_end = (main.segments[0].offset + main.segments[0].size) as usize;
    let words: Vec<u32> = main.image[..text_end.min(main.image.len())]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect();
    let n = words.len();
    let code: Vec<(&sites::Site, &sites::CodeSite)> = sites.iter().filter_map(|s| s.code().map(|c| (s, c))).collect();
    // (site, signature at (word index), entry, the words it needs as they are)
    let mut placed: Vec<Placed> = Vec::new();
    let mut entries = BTreeMap::new();
    let guarded = |c: &sites::CodeSite, at: Option<usize>, entry: i64| {
        let mut out = std::collections::BTreeSet::new();
        if let (Some(at), Some(sig)) = (at, &c.signature) {
            out.extend((at as i64)..(at + sig.len()) as i64);
        }
        if let Some(off) = c.offset {
            out.insert(entry + if c.form.at_offset() { (off / 4) as i64 } else { 0 });
        }
        for g in &c.guards {
            let start = entry + (g.offset / 4) as i64;
            out.extend(start..start + g.signature.len() as i64);
        }
        if c.form == sites::Form::Jump
            && let Some(to) = c.to
        {
            out.insert(entry + (to / 4) as i64);
        }
        out
    };
    for &(s, c) in &code {
        let Some(sig) = &c.signature else { continue };
        let hits = sig.find(&words, 0, 2);
        if hits.len() == 1 {
            let at = (hits[0] / 4) as usize;
            let entry = at as i64 - c.signature_offset / 4;
            entries.insert(s.name.clone(), entry);
            placed.push((s, c, Some(at), entry, guarded(c, Some(at), entry)));
        }
    }
    for &(s, c) in &code {
        if let Some(inside) = &c.inside
            && let Some(&e) = entries.get(inside)
        {
            placed.push((s, c, None, e, guarded(c, None, e)));
        }
    }
    let mut refused = std::collections::BTreeSet::new();
    for (i, f) in files.iter_mut().enumerate() {
        if !f.kind.applies() {
            continue;
        }
        let mut new: BTreeMap<i64, u32> = BTreeMap::new();
        for &(off, byte) in &f.writes {
            let k = off as usize;
            if k < 4 * n {
                let w = new.get(&((k / 4) as i64)).copied().unwrap_or(words[k / 4]);
                let shift = 8 * (k % 4);
                new.insert((k / 4) as i64, (w & !(0xff << shift)) | (u32::from(byte) << shift));
            }
        }
        new.retain(|&k, w| *w != words[k as usize]);
        if new.is_empty() {
            continue;
        }
        let word_at = |t: usize| new.get(&(t as i64)).copied().unwrap_or(words[t]);
        let mut why = None;
        for (s, c, at, e, need) in &placed {
            if let Some(hit) = need.iter().find(|k| new.contains_key(k)) {
                why = Some(format!(
                    "it changes f_{:x}+{:#x}, the code the site is found by or hooked at: it would stop the game's hook site {} \
                     (docs/dev/reference/hooks.md)",
                    c.function,
                    4 * (hit - e),
                    s.name
                ));
                break;
            }
            let (Some(at), Some(sig)) = (at, &c.signature) else { continue };
            let starts: std::collections::BTreeSet<i64> =
                new.keys().flat_map(|&k| (0..sig.len() as i64).map(move |j| k - j)).collect();
            let second = starts.iter().any(|&t| {
                t >= 0
                    && t as usize != *at
                    && t as usize + sig.len() <= n
                    && sig.words().iter().enumerate().all(|(j, m)| m.matches(word_at(t as usize + j)))
            });
            if second {
                why = Some(format!(
                    "it makes the site's signature match a second place, so the site cannot be found: it would stop the game's \
                     hook site {} (docs/dev/reference/hooks.md)",
                    s.name
                ));
                break;
            }
        }
        if let Some(note) = why {
            f.kind = Kind::Refused;
            f.note = note;
            refused.insert(i);
        }
    }
    Ok(refused.into_iter().collect())
}

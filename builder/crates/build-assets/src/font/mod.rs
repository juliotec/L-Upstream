//! An OpenType font with CFF outlines, read and written again with glyphs added: the game's button-icon font with
//! every controller's icons (`prompts`).
//!
//! The font is kept as it is but for what the new glyphs change: the CFF table (its charstrings, charset, names and
//! bounds), `hmtx`, `hhea`, `maxp`, `head` (bounds, checksum), `OS/2`'s first and last character, and `cmap`, which
//! is written anew (one format 4 subtable, [`cmap4`]). Outlines are read with `ttf-parser`.

pub mod outline;

use build_core::{Error, Result};
use outline::Outline;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::path::Path;

fn be16(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn be32(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn bad(detail: &str) -> Error {
    Error::format("font", detail)
}

/// A CFF INDEX at `at`: its items' ranges in `d`, and where it ends.
fn index(d: &[u8], at: usize) -> Result<(Vec<std::ops::Range<usize>>, usize)> {
    let count = usize::from(be16(d, at).ok_or_else(|| bad("an INDEX past the end"))?);
    if count == 0 {
        return Ok((Vec::new(), at + 2));
    }
    let size = usize::from(*d.get(at + 2).ok_or_else(|| bad("an INDEX past the end"))?);
    if !(1..=4).contains(&size) {
        return Err(bad("an INDEX of offsets of no size this reads"));
    }
    let off = |i: usize| -> Result<usize> {
        let p = at + 3 + i * size;
        let b = d.get(p..p + size).ok_or_else(|| bad("an INDEX past the end"))?;
        Ok(b.iter().fold(0usize, |a, &x| a << 8 | usize::from(x)))
    };
    let data = at + 3 + (count + 1) * size - 1;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (a, b) = (data + off(i)?, data + off(i + 1)?);
        if a > b || b > d.len() {
            return Err(bad("an INDEX item past the end"));
        }
        out.push(a..b);
    }
    let end = data + off(count)?;
    Ok((out, end))
}

/// A CFF INDEX of `items`.
fn write_index(items: &[&[u8]]) -> Vec<u8> {
    let mut out = (items.len() as u16).to_be_bytes().to_vec();
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(|i| i.len()).sum::<usize>() + 1;
    let size = if total < 1 << 8 {
        1
    } else if total < 1 << 16 {
        2
    } else if total < 1 << 24 {
        3
    } else {
        4
    };
    out.push(size as u8);
    let mut o = 1usize;
    for i in 0..=items.len() {
        out.extend_from_slice(&(o as u32).to_be_bytes()[4 - size..]);
        if i < items.len() {
            o += items[i].len();
        }
    }
    for i in items {
        out.extend_from_slice(i);
    }
    out
}

/// A DICT's entries: (operator, its operands' bytes as they were, the operands as numbers).
type Dict = Vec<(u16, Vec<u8>, Vec<f64>)>;

fn parse_dict(d: &[u8]) -> Result<Dict> {
    let mut out = Vec::new();
    let (mut at, mut start, mut nums) = (0, 0, Vec::new());
    while at < d.len() {
        let b0 = d[at];
        let get = |k: usize| d.get(at + k).copied().map(i32::from).ok_or_else(|| bad("a DICT cut short"));
        match b0 {
            0..=21 => {
                let (op, len) = if b0 == 12 { (1200 + get(1)? as u16, 2) } else { (u16::from(b0), 1) };
                out.push((op, d[start..at].to_vec(), std::mem::take(&mut nums)));
                at += len;
                start = at;
            }
            28 => {
                nums.push(f64::from(i16::from_be_bytes([get(1)? as u8, get(2)? as u8])));
                at += 3;
            }
            29 => {
                nums.push(f64::from(i32::from_be_bytes([get(1)? as u8, get(2)? as u8, get(3)? as u8, get(4)? as u8])));
                at += 5;
            }
            30 => {
                // (a real: its nibbles up to 0xf; only kept as bytes, its value not needed)
                nums.push(0.0);
                at += 1;
                while at < d.len() && d[at] & 0x0F != 0x0F && d[at] >> 4 != 0x0F {
                    at += 1;
                }
                at += 1;
            }
            32..=246 => {
                nums.push(f64::from(i32::from(b0) - 139));
                at += 1;
            }
            247..=250 => {
                nums.push(f64::from((i32::from(b0) - 247) * 256 + get(1)? + 108));
                at += 2;
            }
            251..=254 => {
                nums.push(f64::from(-(i32::from(b0) - 251) * 256 - get(1)? - 108));
                at += 2;
            }
            _ => return Err(bad("a DICT with a reserved byte")),
        }
    }
    Ok(out)
}

/// A DICT number as 5 bytes, whatever its size: offsets can be written before they are known.
fn int5(v: i64) -> Vec<u8> {
    let mut o = vec![29];
    o.extend_from_slice(&(v as i32).to_be_bytes());
    o
}

fn write_dict(dict: &Dict, set: &BTreeMap<u16, Vec<i64>>) -> Vec<u8> {
    let mut out = Vec::new();
    let put = |op: u16, operands: &[u8], out: &mut Vec<u8>| {
        out.extend_from_slice(operands);
        if op >= 1200 {
            out.extend([12, (op - 1200) as u8]);
        } else {
            out.push(op as u8);
        }
    };
    for (op, raw, _) in dict {
        match set.get(op) {
            Some(v) => put(*op, &v.iter().flat_map(|&x| int5(x)).collect::<Vec<u8>>(), &mut out),
            None => put(*op, raw, &mut out),
        }
    }
    for (op, v) in set {
        if !dict.iter().any(|(o, _, _)| o == op) {
            put(*op, &v.iter().flat_map(|&x| int5(x)).collect::<Vec<u8>>(), &mut out);
        }
    }
    out
}

/// A glyph added.
struct Added {
    name: String,
    charstring: Vec<u8>,
    advance: u16,
    bounds: Option<(f64, f64, f64, f64)>,
}

/// A font being added to.
pub struct FontEdit<'a> {
    data: &'a [u8],
    face: ttf_parser::Face<'a>,
    tables: BTreeMap<[u8; 4], &'a [u8]>,
    added: Vec<Added>,
    default_width: i32,
    nominal_width: i32,
}

/// The most glyphs a font may be given (the prompts add a few hundred): its glyphs and these fit 65,535.
const MOST_ADDED: u16 = 4096;

/// The standard strings of CFF: names below 391 are these, not in the String INDEX.
const STANDARD_STRINGS: u16 = 391;

impl<'a> FontEdit<'a> {
    /// The font `data` (OpenType with a CFF table, not CID-keyed).
    pub fn new(data: &'a [u8]) -> Result<FontEdit<'a>> {
        let face = ttf_parser::Face::parse(data, 0).map_err(|e| bad(&format!("not a font this reads ({e})")))?;
        let n = usize::from(be16(data, 4).ok_or_else(|| bad("cut short"))?);
        let mut tables = BTreeMap::new();
        for i in 0..n {
            let rec = 12 + 16 * i;
            let tag: [u8; 4] = data.get(rec..rec + 4).ok_or_else(|| bad("cut short"))?.try_into().expect("4 bytes");
            let (off, len) = (be32(data, rec + 8).unwrap_or(0) as usize, be32(data, rec + 12).unwrap_or(0) as usize);
            let table = off.checked_add(len).and_then(|end| data.get(off..end));
            // (two tables of one tag: which one ttf-parser read is not the one written)
            if tables.insert(tag, table.ok_or_else(|| bad("a table past the end"))?).is_some() {
                return Err(bad("two tables of one tag"));
            }
        }
        // (the tables written into in place, each as long as what is written there)
        for (tag, least) in [(b"hhea", 36), (b"maxp", 6), (b"head", 54)] {
            if tables.get(tag).is_none_or(|t| t.len() < least) {
                return Err(bad(&format!("no {} table, or one cut short", String::from_utf8_lossy(tag))));
            }
        }
        if face.number_of_glyphs() > u16::MAX - MOST_ADDED {
            return Err(bad("a font of too many glyphs to add to"));
        }
        let mut edit = FontEdit { data, face, tables, added: Vec::new(), default_width: 0, nominal_width: 0 };
        let cff = edit.cff()?;
        edit.default_width = cff.private_number(20).unwrap_or(0.0) as i32;
        edit.nominal_width = cff.private_number(21).unwrap_or(0.0) as i32;
        Ok(edit)
    }

    /// The font as read.
    pub fn face(&self) -> &ttf_parser::Face<'a> {
        &self.face
    }

    /// Its glyphs, those added too.
    pub fn glyph_count(&self) -> u16 {
        self.face.number_of_glyphs().saturating_add(u16::try_from(self.added.len()).unwrap_or(u16::MAX))
    }

    /// A glyph's advance (an added one's too).
    pub fn advance(&self, glyph: u16) -> u16 {
        let base = self.face.number_of_glyphs();
        if glyph >= base {
            return self.added[usize::from(glyph - base)].advance;
        }
        self.face.glyph_hor_advance(ttf_parser::GlyphId(glyph)).unwrap_or(0)
    }

    /// A glyph's outline (an added one's is not kept: only the font's own are asked for).
    pub fn outline(&self, glyph: u16) -> Outline {
        Outline::of(&self.face, ttf_parser::GlyphId(glyph))
    }

    /// The font's code points and their glyphs (its best Unicode subtable).
    pub fn cmap(&self) -> BTreeMap<u32, u16> {
        let mut out = BTreeMap::new();
        let Some(cmap) = self.face.tables().cmap else { return out };
        let rank = |p: ttf_parser::PlatformId, e: u16| match (p, e) {
            (ttf_parser::PlatformId::Windows, 10) => 0,
            (ttf_parser::PlatformId::Unicode, 6) => 1,
            (ttf_parser::PlatformId::Unicode, 4) => 2,
            (ttf_parser::PlatformId::Windows, 1) => 3,
            (ttf_parser::PlatformId::Unicode, 3) => 4,
            (ttf_parser::PlatformId::Unicode, _) => 5,
            _ => 9,
        };
        let best = cmap
            .subtables
            .into_iter()
            .filter(|s| rank(s.platform_id, s.encoding_id) < 9)
            .min_by_key(|s| rank(s.platform_id, s.encoding_id));
        if let Some(s) = best {
            s.codepoints(|c| {
                if let Some(g) = s.glyph_index(c) {
                    out.insert(c, g.0);
                }
            });
        }
        out
    }

    /// The glyph named `name`, if the font has it.
    pub fn glyph_by_name(&self, name: &str) -> Option<u16> {
        self.face.glyph_index_by_name(name).map(|g| g.0)
    }

    /// `outline` added as a glyph named `name`, `advance` wide: its id.
    /// (no more than [`MOST_ADDED`]: one past them is refused when the font is written)
    pub fn add(&mut self, name: &str, outline: &Outline, advance: i32) -> u16 {
        let charstring = outline.charstring(advance, self.default_width, self.nominal_width);
        let bounds = outline
            .bounds()
            .map(|(x0, y0, x1, y1)| ((x0 + 0.5).floor(), (y0 + 0.5).floor(), (x1 + 0.5).floor(), (y1 + 0.5).floor()));
        self.added.push(Added { name: name.into(), charstring, advance: advance.clamp(0, 65535) as u16, bounds });
        self.glyph_count() - 1
    }

    fn cff(&self) -> Result<Cff<'a>> {
        Cff::parse(self.tables.get(b"CFF ").ok_or_else(|| bad("no CFF table"))?)
    }

    fn bounds(&self, glyph: u16) -> Option<(f64, f64, f64, f64)> {
        let base = self.face.number_of_glyphs();
        if glyph >= base {
            return self.added[usize::from(glyph - base)].bounds;
        }
        self.face
            .glyph_bounding_box(ttf_parser::GlyphId(glyph))
            .map(|r| (f64::from(r.x_min), f64::from(r.y_min), f64::from(r.x_max), f64::from(r.y_max)))
    }

    /// The font with the glyphs added and `mapping` (code point: glyph) as its cmap ([`cmap4`]; each code of
    /// `singles` a segment of its own).
    pub fn write(&self, mapping: &BTreeMap<u32, u16>, singles: &BTreeSet<u32>) -> Result<Vec<u8>> {
        if self.added.len() > usize::from(MOST_ADDED) {
            return Err(bad("too many glyphs added"));
        }
        let n = self.glyph_count();
        let mut bbox: Option<(f64, f64, f64, f64)> = None;
        let mut metrics = Vec::with_capacity(usize::from(n));
        let hmtx = self.tables.get(b"hmtx").copied().unwrap_or_default();
        let hhea = self.tables.get(b"hhea").ok_or_else(|| bad("no hhea table"))?;
        let long = usize::from(be16(hhea, 34).unwrap_or(0));
        for g in 0..n {
            let b = self.bounds(g);
            if let Some((x0, y0, x1, y1)) = b {
                bbox = Some(bbox.map_or((x0, y0, x1, y1), |(a, c, e, f)| (a.min(x0), c.min(y0), e.max(x1), f.max(y1))));
            }
            let lsb = if g < self.face.number_of_glyphs() {
                let at =
                    if usize::from(g) < long { 4 * usize::from(g) + 2 } else { 4 * long + 2 * (usize::from(g) - long) };
                be16(hmtx, at).map_or(0, |v| v as i16)
            } else {
                b.map_or(0, |b| b.0 as i16)
            };
            metrics.push((self.advance(g), lsb, b));
        }
        let (bx0, by0, bx1, by1) = bbox.unwrap_or_default();
        let cff = self.cff()?.with_glyphs(&self.added, [bx0, by0, bx1, by1].map(|v| v as i64))?;
        let mut hmtx_out = Vec::with_capacity(metrics.len() * 4);
        for (adv, lsb, _) in &metrics {
            hmtx_out.extend_from_slice(&adv.to_be_bytes());
            hmtx_out.extend_from_slice(&lsb.to_be_bytes());
        }
        let mut hhea_out = hhea.to_vec();
        let max_adv = metrics.iter().map(|m| m.0).max().unwrap_or(0);
        let with_outline = || metrics.iter().filter_map(|m| m.2.map(|b| (m.0, m.1, b)));
        let min_lsb = with_outline().map(|m| m.1).min().unwrap_or(0);
        let min_rsb =
            with_outline().map(|(a, l, b)| i32::from(a) - i32::from(l) - (b.2 - b.0) as i32).min().unwrap_or(0);
        let max_extent = with_outline().map(|(_, l, b)| i32::from(l) + (b.2 - b.0) as i32).max().unwrap_or(0);
        hhea_out[10..12].copy_from_slice(&max_adv.to_be_bytes());
        hhea_out[12..14].copy_from_slice(&min_lsb.to_be_bytes());
        hhea_out[14..16].copy_from_slice(&(min_rsb as i16).to_be_bytes());
        hhea_out[16..18].copy_from_slice(&(max_extent as i16).to_be_bytes());
        hhea_out[34..36].copy_from_slice(&n.to_be_bytes());
        let mut maxp = self.tables.get(b"maxp").ok_or_else(|| bad("no maxp table"))?.to_vec();
        maxp[4..6].copy_from_slice(&n.to_be_bytes());
        let mut head = self.tables.get(b"head").ok_or_else(|| bad("no head table"))?.to_vec();
        for (k, v) in [bx0, by0, bx1, by1].iter().enumerate() {
            head[36 + 2 * k..38 + 2 * k].copy_from_slice(&(*v as i16).to_be_bytes());
        }
        head[8..12].copy_from_slice(&[0; 4]);
        let mut tables: BTreeMap<[u8; 4], Vec<u8>> = self.tables.iter().map(|(t, d)| (*t, d.to_vec())).collect();
        if let Some(os2) = tables.get_mut(b"OS/2")
            && os2.len() >= 68
        {
            let codes: Vec<u32> = mapping.keys().copied().filter(|&c| c <= 0xFFFF).collect();
            let (first, last) = (codes.first().copied().unwrap_or(0), codes.last().copied().unwrap_or(0));
            os2[64..66].copy_from_slice(&(first as u16).to_be_bytes());
            os2[66..68].copy_from_slice(&(last as u16).to_be_bytes());
        }
        tables.insert(*b"CFF ", cff);
        tables.insert(*b"hmtx", hmtx_out);
        tables.insert(*b"hhea", hhea_out);
        tables.insert(*b"maxp", maxp);
        tables.insert(*b"head", head);
        tables.insert(*b"cmap", cmap4(mapping, singles));
        Ok(sfnt(be32(self.data, 0).unwrap_or(0x4F54_544F), &tables))
    }
}

fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(w))
    })
}

/// An sfnt of `tables` (by tag), each 4-aligned, `head`'s checksum adjustment set.
fn sfnt(version: u32, tables: &BTreeMap<[u8; 4], Vec<u8>>) -> Vec<u8> {
    let n = tables.len() as u16;
    let entry = 15 - n.leading_zeros() as u16; // (log2 of the largest power of 2 at most n)
    let search = 16u16 << entry;
    let mut out = version.to_be_bytes().to_vec();
    for v in [n, search, entry, n * 16 - search] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    let mut at = 12 + 16 * tables.len();
    let mut body = Vec::new();
    let mut head_at = None;
    for (tag, d) in tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(d).to_be_bytes());
        out.extend_from_slice(&(at as u32).to_be_bytes());
        out.extend_from_slice(&(d.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(at);
        }
        body.extend_from_slice(d);
        body.resize(body.len().next_multiple_of(4), 0);
        at = 12 + 16 * tables.len() + body.len();
    }
    out.extend(body);
    if let Some(h) = head_at {
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[h + 8..h + 12].copy_from_slice(&adjust.to_be_bytes());
    }
    out
}

/// A cmap table of one format 4 subtable (Unicode BMP, for platforms 0 and 3) from `mapping` (code: glyph): runs of
/// codes with consecutive glyphs share a segment, each code of `single` has one of its own, every segment by its
/// idDelta alone (`host/prompts.c` points the single ones at other glyphs as the game runs).
pub fn cmap4(mapping: &BTreeMap<u32, u16>, single: &BTreeSet<u32>) -> Vec<u8> {
    let mut segs: Vec<[u32; 3]> = Vec::new();
    for (&c, &g) in mapping.range(..=0xFFFF) {
        let g = u32::from(g);
        match segs.last_mut() {
            Some(s) if !single.contains(&c) && s[1] + 1 == c && !single.contains(&s[1]) && (s[2] + c) & 0xFFFF == g => {
                s[1] = c;
            }
            _ => segs.push([c, c, g.wrapping_sub(c) & 0xFFFF]),
        }
    }
    segs.push([0xFFFF, 0xFFFF, 1]);
    let n = segs.len() as u32;
    let search = 2 * (1u32 << (31 - n.leading_zeros()));
    let mut sub = Vec::new();
    for v in [4, 16 + 8 * n, 0, 2 * n, search, (search / 2).ilog2(), 2 * n - search] {
        sub.extend_from_slice(&(v as u16).to_be_bytes());
    }
    sub.extend(segs.iter().flat_map(|s| (s[1] as u16).to_be_bytes()));
    sub.extend_from_slice(&[0, 0]);
    sub.extend(segs.iter().flat_map(|s| (s[0] as u16).to_be_bytes()));
    sub.extend(segs.iter().flat_map(|s| (s[2] as u16).to_be_bytes()));
    sub.extend(std::iter::repeat_n(0u8, 2 * segs.len()));
    let mut out = Vec::new();
    for v in [0u16, 2, 0, 3] {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&20u32.to_be_bytes());
    out.extend_from_slice(&[0, 3, 0, 1]);
    out.extend_from_slice(&20u32.to_be_bytes());
    out.extend(sub);
    out
}

/// A CFF table, as read.
struct Cff<'a> {
    header: &'a [u8],
    name_index: &'a [u8],
    top: Dict,
    strings: Vec<&'a [u8]>,
    gsubrs: &'a [u8],
    charstrings: Vec<&'a [u8]>,
    charset: Vec<u16>,
    private: Dict,
    subrs: Option<&'a [u8]>,
}

impl<'a> Cff<'a> {
    fn parse(d: &'a [u8]) -> Result<Cff<'a>> {
        let hdr = usize::from(*d.get(2).ok_or_else(|| bad("a CFF table cut short"))?);
        let (names, at) = index(d, hdr)?;
        let (tops, at2) = index(d, at)?;
        let (strings, at3) = index(d, at2)?;
        let (_, at4) = index(d, at3)?;
        if names.len() != 1 || tops.len() != 1 {
            return Err(bad("a CFF table of more than one font"));
        }
        let top = parse_dict(&d[tops[0].clone()])?;
        let num = |op: u16| top.iter().find(|e| e.0 == op).map(|e| e.2.clone());
        if num(1230).is_some() {
            return Err(bad("a CID-keyed CFF table"));
        }
        let cs_at = num(17).and_then(|v| v.first().copied()).ok_or_else(|| bad("no CharStrings"))? as usize;
        let (charstrings, _) = index(d, cs_at)?;
        let n = charstrings.len();
        if n == 0 {
            return Err(bad("a CFF table of no glyphs"));
        }
        let mut charset = vec![0u16];
        let set_at = num(15).and_then(|v| v.first().copied()).unwrap_or(0.0) as usize;
        if set_at > 2 {
            let format = d.get(set_at).copied();
            let mut p = set_at + 1;
            while charset.len() < n {
                match format {
                    Some(0) => {
                        charset.push(be16(d, p).ok_or_else(|| bad("a charset cut short"))?);
                        p += 2;
                    }
                    Some(1) | Some(2) => {
                        let first = be16(d, p).ok_or_else(|| bad("a charset cut short"))?;
                        let left = if format == Some(1) {
                            u16::from(*d.get(p + 2).ok_or_else(|| bad("a charset cut short"))?)
                        } else {
                            be16(d, p + 2).ok_or_else(|| bad("a charset cut short"))?
                        };
                        p += if format == Some(1) { 3 } else { 4 };
                        for k in 0..=left {
                            charset.push(first.checked_add(k).ok_or_else(|| bad("a charset past 65535"))?);
                        }
                    }
                    _ => return Err(bad("a charset of an unknown format")),
                }
            }
            charset.truncate(n);
        } else {
            return Err(bad("a predefined charset"));
        }
        let pv = num(18).ok_or_else(|| bad("no Private DICT"))?;
        let (psize, poff) = match (pv.first(), pv.get(1)) {
            (Some(&size), Some(&off)) => (size as usize, off as usize),
            _ => return Err(bad("a Private DICT without its size and place")),
        };
        let private = parse_dict(
            poff.checked_add(psize)
                .and_then(|end| d.get(poff..end))
                .ok_or_else(|| bad("a Private DICT past the end"))?,
        )?;
        let subrs = match private.iter().find(|e| e.0 == 19) {
            Some(e) => {
                let rel = *e.2.first().ok_or_else(|| bad("Subrs without its place"))?;
                let at = poff.checked_add(rel as usize).ok_or_else(|| bad("Subrs past the end"))?;
                let (_, end) = index(d, at)?;
                Some(&d[at..end])
            }
            None => None,
        };
        Ok(Cff {
            header: &d[..hdr],
            name_index: &d[hdr..at],
            top,
            strings: strings.into_iter().map(|r| &d[r]).collect(),
            gsubrs: &d[at3..at4],
            charstrings: charstrings.into_iter().map(|r| &d[r]).collect(),
            charset,
            private,
            subrs,
        })
    }

    fn private_number(&self, op: u16) -> Option<f64> {
        self.private.iter().find(|e| e.0 == op).and_then(|e| e.2.first().copied())
    }

    /// The table again with `added`'s glyphs after its own, and `bbox` as its FontBBox.
    fn with_glyphs(&self, added: &[Added], bbox: [i64; 4]) -> Result<Vec<u8>> {
        let mut strings: Vec<Vec<u8>> = self.strings.iter().map(|s| s.to_vec()).collect();
        let mut charset = self.charset.clone();
        for a in added {
            let sid = STANDARD_STRINGS as usize + strings.len();
            strings.push(a.name.as_bytes().to_vec());
            charset.push(u16::try_from(sid).map_err(|_| bad("too many glyph names"))?);
        }
        let mut charstrings: Vec<&[u8]> = self.charstrings.clone();
        charstrings.extend(added.iter().map(|a| a.charstring.as_slice()));
        let string_index = write_index(&strings.iter().map(Vec::as_slice).collect::<Vec<_>>());
        let charset_bytes: Vec<u8> =
            std::iter::once(0u8).chain(charset[1..].iter().flat_map(|s| s.to_be_bytes())).collect();
        let cs_index = write_index(&charstrings);
        // (the Private DICT with its Subrs offset as 5 bytes: the same length whatever it is)
        let probe: BTreeMap<u16, Vec<i64>> = self.subrs.iter().map(|_| (19u16, vec![0])).collect();
        let private_len = write_dict(&self.private, &probe).len();
        let private_set: BTreeMap<u16, Vec<i64>> =
            self.subrs.iter().map(|_| (19u16, vec![private_len as i64])).collect();
        let private = write_dict(&self.private, &private_set);
        // (the top DICT, its offsets as 5 bytes, so its length is known before they are)
        let top_set = |charset_at: i64, cs_at: i64, private_at: i64| -> BTreeMap<u16, Vec<i64>> {
            BTreeMap::from([
                (5, bbox.to_vec()),
                (15, vec![charset_at]),
                (17, vec![cs_at]),
                (18, vec![private.len() as i64, private_at]),
            ])
        };
        let top_len = write_index(&[&write_dict(&self.top, &top_set(0, 0, 0))]).len();
        let start = self.header.len() + self.name_index.len() + top_len + string_index.len() + self.gsubrs.len();
        let charset_at = start;
        let cs_at = charset_at + charset_bytes.len();
        let private_at = cs_at + cs_index.len();
        let top = write_index(&[&write_dict(&self.top, &top_set(charset_at as i64, cs_at as i64, private_at as i64))]);
        debug_assert_eq!(top.len(), top_len);
        let mut out = Vec::new();
        for part in
            [self.header, self.name_index, &top, &string_index, self.gsubrs, &charset_bytes, &cs_index, &private]
        {
            out.extend_from_slice(part);
        }
        if let Some(s) = self.subrs {
            out.extend_from_slice(s);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font of the repository's (Kenney's, CC0): no game file in a test.
    fn kenney() -> Vec<u8> {
        let p =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../third_party/kenney_input/kenney_input_steam_deck.otf");
        std::fs::read(p).expect("Kenney's font in third_party")
    }

    /// A CFF table of `glyphs` charstrings, its Private DICT's operands `private` (its size and place, the place filled
    /// in when one is given), its charset format 0.
    fn cff(glyphs: usize, private: &[Option<i64>]) -> Vec<u8> {
        let names = write_index(&[b"F"]);
        let empty = write_index(&[]);
        let dict_at = |charset: i64, strings_at: i64, private_at: i64| {
            let mut d = int5(charset);
            d.push(15);
            d.extend(int5(strings_at));
            d.push(17);
            for v in private {
                d.extend(int5(v.unwrap_or(private_at)));
            }
            d.push(18);
            d
        };
        let top_len = write_index(&[&dict_at(0, 0, 0)]).len();
        let charset_at = 4 + names.len() + top_len + 2 * empty.len();
        let charset: Vec<u8> =
            std::iter::once(0u8).chain((1..glyphs.max(1) as u16).flat_map(|g| g.to_be_bytes())).collect();
        let cs_at = charset_at + charset.len();
        let items: Vec<&[u8]> = (0..glyphs).map(|_| &[14u8][..]).collect();
        let charstrings = write_index(&items);
        let private_at = cs_at + charstrings.len();
        let mut out = vec![1, 0, 4, 1];
        out.extend(&names);
        out.extend(write_index(&[&dict_at(charset_at as i64, cs_at as i64, private_at as i64)]));
        out.extend(&empty);
        out.extend(&empty);
        out.extend(&charset);
        out.extend(&charstrings);
        out.extend([139, 20]); // (a Private DICT: defaultWidthX 0)
        out
    }

    #[test]
    fn a_crafted_cff_table_is_refused_not_read_past() {
        assert!(Cff::parse(&cff(2, &[Some(2), None])).is_ok(), "the crafted table reads when it is whole");
        // (a Private DICT with one operand of two: before, its second was taken past the end of the operands)
        assert!(Cff::parse(&cff(2, &[Some(2)])).is_err());
        // (no glyphs at all: none to add after)
        assert!(Cff::parse(&cff(0, &[Some(2), None])).is_err());
    }

    #[test]
    fn two_tables_of_one_tag_and_a_table_cut_short_are_refused() {
        let data = kenney();
        let n = usize::from(be16(&data, 4).unwrap());
        let tag_of = |d: &[u8], i: usize| d[12 + 16 * i..16 + 16 * i].to_vec();
        let find = |d: &[u8], t: &[u8; 4]| (0..n).find(|&i| tag_of(d, i) == t).unwrap();
        // (hhea's length made 10)
        let mut short = data.clone();
        let h = find(&short, b"hhea");
        short[12 + 16 * h + 12..12 + 16 * h + 16].copy_from_slice(&10u32.to_be_bytes());
        assert!(FontEdit::new(&short).is_err());
        // (the post table's record given the name table's tag: two of it)
        let mut twice = data.clone();
        let (post, name) = (find(&twice, b"post"), find(&twice, b"name"));
        let tag = tag_of(&twice, name);
        twice[12 + 16 * post..16 + 16 * post].copy_from_slice(&tag);
        assert!(FontEdit::new(&twice).is_err());
    }

    #[test]
    fn a_glyph_added_reads_back_with_its_outline_and_code() {
        let data = kenney();
        let mut f = FontEdit::new(&data).unwrap();
        let n = f.glyph_count();
        let src = f.glyph_by_name("steamdeck_button_a").unwrap();
        let o = f.outline(src).transformed(0.5, 10.0, 20.0);
        let g = f.add("made_a", &o, 777);
        assert_eq!(g, n);
        let mut mapping = f.cmap();
        mapping.insert(0xF700, g);
        let out = f.write(&mapping, &BTreeSet::from([0xF700])).unwrap();
        assert_eq!(f.write(&mapping, &BTreeSet::from([0xF700])).unwrap(), out, "the same bytes each time");
        let back = ttf_parser::Face::parse(&out, 0).unwrap();
        assert_eq!(back.number_of_glyphs(), n + 1);
        let id = back.glyph_index('\u{F700}').unwrap();
        assert_eq!(id.0, g);
        assert_eq!(back.glyph_hor_advance(id), Some(777));
        assert_eq!(back.glyph_name(id), Some("made_a"));
        let round =
            |o: &Outline| o.transformed(1.0, 0.0, 0.0).bounds().map(|b| [b.0, b.1, b.2, b.3].map(|v| v.round()));
        assert_eq!(round(&Outline::of(&back, id)), round(&o));
        // (the font's own glyphs are as they were)
        assert_eq!(Outline::of(&back, ttf_parser::GlyphId(src)), f.outline(src));
    }

    #[test]
    fn a_cmap_s_segments() {
        let mapping = BTreeMap::from([(0x41, 5), (0x42, 6), (0x43, 7), (0x45, 9), (0xE040, 3)]);
        let one = cmap4(&mapping, &BTreeSet::new());
        let apart = cmap4(&mapping, &BTreeSet::from([0x42]));
        // (A..C one segment, E, U+E040, the end: 4; with B apart: 6)
        assert_eq!(u16::from_be_bytes([one[26], one[27]]), 8);
        assert_eq!(u16::from_be_bytes([apart[26], apart[27]]), 12);
    }

    #[test]
    fn an_index_round_trip() {
        let items: Vec<Vec<u8>> = (0..300).map(|i| vec![i as u8; i % 7]).collect();
        let refs: Vec<&[u8]> = items.iter().map(Vec::as_slice).collect();
        let bytes = write_index(&refs);
        let (ranges, end) = index(&bytes, 0).unwrap();
        assert_eq!(end, bytes.len());
        assert!(ranges.iter().zip(&items).all(|(r, i)| &bytes[r.clone()] == i.as_slice()));
    }

    #[test]
    fn a_dict_keeps_what_it_does_not_change() {
        let raw = [139, 140, 141, 142, 5, 0x1e, 0x2a, 0x5f, 12, 7, 247, 0, 17];
        let d = parse_dict(&raw).unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].2, [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(write_dict(&d, &BTreeMap::new()), raw);
        let set = BTreeMap::from([(17u16, vec![1000i64])]);
        assert_eq!(write_dict(&d, &set)[raw.len() - 3..], [29, 0, 0, 3, 232, 17]);
    }
}

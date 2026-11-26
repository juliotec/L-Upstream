//! The game's textures (`UI/Layout2Archive/Common.*.bntx`: ASTC 4x4 and BC4 in the Switch 2's block-linear layout):
//! read, decoded to pictures, and pictures encoded back into the blocks of a texture's place in the file.

pub mod astc;

use crate::raster::Image;
use build_core::{Error, Result};

/// A texture of a BNTX.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    /// Its format (high byte 0x2D: ASTC 4x4; low byte 6: sRGB).
    pub format: u32,
    /// Its width in pixels.
    pub width: usize,
    /// Its height in pixels.
    pub height: usize,
    /// GOBs a block of the layout.
    pub gobs: usize,
    /// Where its data starts in the file.
    pub data: usize,
}

impl Texture {
    /// Whether it is ASTC (else BC4).
    pub fn astc(&self) -> bool {
        self.format >> 8 == 0x2D
    }

    /// Whether its colours are sRGB.
    pub fn srgb(&self) -> bool {
        self.format & 0xFF == 6
    }

    /// Bytes a block.
    pub fn block_bytes(&self) -> usize {
        if self.astc() { 16 } else { 8 }
    }

    /// Its size in blocks.
    pub fn blocks(&self) -> (usize, usize) {
        (self.width.div_ceil(4), self.height.div_ceil(4))
    }

    /// The GOBs' row pitch, in 64-byte units.
    fn pitch(&self) -> usize {
        (self.blocks().0 * self.block_bytes()).div_ceil(64)
    }

    /// Whether its every block is within a file of `len` bytes: its last block is its furthest (the GOBs' stripe,
    /// then column, then the place in a GOB each grow with x and y).
    pub fn within(&self, len: usize) -> bool {
        let (bw, bh) = self.blocks();
        bw > 0
            && bh > 0
            && bw <= MOST_SIDE
            && bh <= MOST_SIDE
            && self
                .data
                .checked_add(gob_offset((bw - 1) * self.block_bytes(), bh - 1, self.gobs, self.pitch()))
                .and_then(|at| at.checked_add(self.block_bytes()))
                .is_some_and(|end| end <= len)
    }

    /// Where block (x, y) is in the file.
    pub fn block_at(&self, x: usize, y: usize) -> usize {
        self.data + gob_offset(x * self.block_bytes(), y, self.gobs, self.pitch())
    }
}

/// The widest and highest texture this reads.
pub const MOST_SIDE: usize = 4096;

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?)))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u64::from_le_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?) as usize)
}

/// The textures of a Switch 2 BNTX (platform "Ounc"), by name.
pub fn bntx_textures(b: &[u8]) -> Result<Vec<(String, Texture)>> {
    let bad = || Error::format("Common.bntx", "not a texture archive this reads");
    let count = u32_at(b, 0x24).ok_or_else(bad)? as usize;
    let info = u64_at(b, 0x28).ok_or_else(bad)?;
    let at = |base: usize, off: usize| base.checked_add(off).ok_or_else(bad);
    let mut out = Vec::with_capacity(count.min(1 << 16));
    for i in 0..count {
        let p = u64_at(b, at(info, 8usize.checked_mul(i).ok_or_else(bad)?)?).ok_or_else(bad)?;
        let no = u64_at(b, at(p, 0x60)?).ok_or_else(bad)?;
        let nl = u16_at(b, no).ok_or_else(bad)?;
        let name = String::from_utf8_lossy(b.get(at(no, 2)?..at(no, 2 + nl)?).ok_or_else(bad)?).into_owned();
        let format = u32_at(b, at(p, 0x1C)?).ok_or_else(bad)?;
        let (w, h) =
            (u32_at(b, at(p, 0x24)?).ok_or_else(bad)? as usize, u32_at(b, at(p, 0x28)?).ok_or_else(bad)? as usize);
        let bh = u32_at(b, at(p, 0x34)?).ok_or_else(bad)?;
        let mips = u64_at(b, at(p, 0x70)?).ok_or_else(bad)?;
        let data = u64_at(b, mips).ok_or_else(bad)?;
        let t = Texture { format, width: w, height: h, gobs: 1 << bh.min(5), data };
        // (a texture this reads is no larger than 4096 x 4096, its last block in the data: another is left out, and
        // when the prompts draw on it, they find it missing (textures::unknown). The game's own has larger ones)
        if !(1..=MOST_SIDE).contains(&w) || !(1..=MOST_SIDE).contains(&h) || !t.within(b.len()) {
            continue;
        }
        out.push((name, t));
    }
    Ok(out)
}

/// A byte's place in a block-linear texture (x in bytes, y in rows of blocks): the Switch 2's GOB, the Tegra X1's
/// with offset bits 5 and 6 swapped (`gpu/render/rx_nvn.cpp bl_offset`).
pub fn gob_offset(x: usize, y: usize, gobs: usize, pitch: usize) -> usize {
    let a = (y / (8 * gobs)) * 512 * gobs * pitch + (x / 64) * 512 * gobs + (y % (8 * gobs) / 8) * 512;
    a + (((x & 63) >> 5) << 8
        | ((y & 7) >> 2) << 7
        | ((x & 31) >> 4) << 6
        | ((y >> 1) & 1) << 5
        | (y & 1) << 4
        | (x & 15))
}

/// BC4 decoded: a block's 16 values.
pub fn bc4_decode(block: &[u8]) -> [u8; 16] {
    let (a0, a1) = (u32::from(block[0]), u32::from(block[1]));
    let mut lut = [a0, a1, 0, 0, 0, 0, 0, 255];
    if a0 > a1 {
        for (i, slot) in lut.iter_mut().enumerate().skip(2) {
            *slot = ((8 - i as u32) * a0 + (i as u32 - 1) * a1) / 7;
        }
    } else {
        for (i, slot) in lut.iter_mut().enumerate().take(6).skip(2) {
            *slot = ((6 - i as u32) * a0 + (i as u32 - 1) * a1) / 5;
        }
    }
    let idx = block[2..8].iter().rev().fold(0u64, |a, &b| a << 8 | u64::from(b));
    std::array::from_fn(|k| lut[((idx >> (3 * k)) & 7) as usize] as u8)
}

/// BC4 by each block's darkest and lightest, the 6 levels between (`nx2/prompt_assets.py encode_bc4`).
pub fn bc4_encode(v: &[u8; 16]) -> [u8; 8] {
    let (hi, lo) = (*v.iter().max().unwrap_or(&0), *v.iter().min().unwrap_or(&0));
    let span = f64::from((hi - lo).max(1));
    let mut idx = 0u64;
    for (k, &x) in v.iter().enumerate() {
        let t = if hi > lo { (f64::from(x - lo) * 7.0 / span).round_ties_even() as u64 } else { 7 };
        // (0 the lightest, 1 the darkest, 2..7 between)
        let code = match t {
            7 => 0,
            0 => 1,
            _ => 8 - t,
        };
        idx |= code << (3 * k);
    }
    let mut out = [hi, lo, 0, 0, 0, 0, 0, 0];
    out[2..].copy_from_slice(&idx.to_le_bytes()[..6]);
    out
}

/// A texture decoded: RGBA for ASTC (decoded as plain LDR, as the Python's decoder did), grey for BC4.
pub fn decode(b: &[u8], t: &Texture) -> Result<Image> {
    let (bw, bh) = t.blocks();
    let bpe = t.block_bytes();
    let mut im = if t.astc() { Image::rgba(bw * 4, bh * 4) } else { Image::grey(bw * 4, bh * 4) };
    for by in 0..bh {
        for bx in 0..bw {
            let at = t.block_at(bx, by);
            let block = at
                .checked_add(bpe)
                .and_then(|end| b.get(at..end))
                .ok_or_else(|| Error::format("Common.bntx", "a texture past the file's end"))?;
            if t.astc() {
                let texels = astc::decode_block(block.try_into().expect("16 bytes"), 4, 4, false);
                for (k, px) in texels.iter().enumerate() {
                    im.px_mut(bx * 4 + k % 4, by * 4 + k / 4).copy_from_slice(px);
                }
            } else {
                for (k, v) in bc4_decode(block).into_iter().enumerate() {
                    im.px_mut(bx * 4 + k % 4, by * 4 + k / 4)[0] = v;
                }
            }
        }
    }
    Ok(im.crop(0, 0, t.width as i64, t.height as i64))
}

/// A run of bytes to write over the file: (offset, bytes).
pub type Run = (usize, Vec<u8>);

/// One block's encoder: 16 texels (row by row) and whether the texture is sRGB, to the block's bytes.
pub type Encoder<'a> = dyn Fn(&[[u8; 4]], bool) -> Vec<u8> + 'a;

/// The blocks of `im` (the texture's size; RGBA for ASTC, grey for BC4) inside `bx` (pixels, (x0, y0, x1, y1)), or
/// those of them with any of `mask` (grey, `im`'s size) when no box is given, encoded as runs of the file: blocks
/// next to each other in the file are one run.
pub fn runs(
    t: &Texture,
    im: &Image,
    bx: Option<(i64, i64, i64, i64)>,
    mask: Option<&Image>,
    encode: &Encoder<'_>,
) -> Vec<Run> {
    let (bw, bh) = t.blocks();
    let (x0, y0, x1, y1) = bx
        .or_else(|| mask.and_then(Image::bbox).map(|(a, b, c, d)| (a as i64, b as i64, c as i64, d as i64)))
        .unwrap_or((0, 0, t.width as i64, t.height as i64));
    let (bx0, by0) = (x0.div_euclid(4).max(0) as usize, y0.div_euclid(4).max(0) as usize);
    let (bx1, by1) =
        ((x1 + 3).div_euclid(4).clamp(0, bw as i64) as usize, (y1 + 3).div_euclid(4).clamp(0, bh as i64) as usize);
    let mut pieces: Vec<Run> = Vec::new();
    for by in by0..by1 {
        for bx in bx0..bx1 {
            let covered = mask.is_none_or(|m| {
                (0..16).any(|k| {
                    let (x, y) = (bx * 4 + k % 4, by * 4 + k / 4);
                    x < m.width && y < m.height && m.at(x, y) > 0
                })
            });
            if !covered {
                continue;
            }
            let texels: Vec<[u8; 4]> = (0..16)
                .map(|k| {
                    let (x, y) = (bx * 4 + k % 4, by * 4 + k / 4);
                    if x < im.width && y < im.height {
                        let p = im.px(x, y);
                        if p.len() == 4 { [p[0], p[1], p[2], p[3]] } else { [p[0], p[0], p[0], 255] }
                    } else {
                        [0; 4] // (past the picture's edge, in the last blocks: nothing)
                    }
                })
                .collect();
            pieces.push((t.block_at(bx, by), encode(&texels, t.srgb())));
        }
    }
    pieces.sort_by_key(|p| p.0);
    let mut out: Vec<Run> = Vec::new();
    for (off, piece) in pieces {
        match out.last_mut() {
            Some(last) if last.0 + last.1.len() == off => last.1.extend(piece),
            _ => out.push((off, piece)),
        }
    }
    out
}

/// One block encoded in `t`'s format: ASTC from RGBA, BC4 from the first channel.
pub fn encode_for(t: &Texture, texels: &[[u8; 4]], srgb: bool) -> Vec<u8> {
    if t.astc() {
        astc::encode_block(texels, srgb).to_vec()
    } else {
        let v: [u8; 16] = std::array::from_fn(|k| texels[k][0]);
        bc4_encode(&v).to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bc4_round_trip_keeps_the_extremes() {
        let v: [u8; 16] = std::array::from_fn(|k| (k * 17) as u8);
        let back = bc4_decode(&bc4_encode(&v));
        assert_eq!((back[0], back[15]), (0, 255));
        assert!(v.iter().zip(&back).all(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() <= 19));
        let flat = [77u8; 16];
        assert_eq!(bc4_decode(&bc4_encode(&flat)), flat);
    }

    /// A BNTX of one texture "T": its format, size and data offset.
    fn bntx_of(format: u32, w: u32, h: u32, data: u64, len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len.max(0x200)];
        b[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
        b[0x28..0x30].copy_from_slice(&0x40u64.to_le_bytes()); // (the info table)
        b[0x40..0x48].copy_from_slice(&0x80u64.to_le_bytes()); // (texture 0's record)
        let p = 0x80;
        b[p + 0x60..p + 0x68].copy_from_slice(&0x180u64.to_le_bytes()); // (its name)
        b[0x180..0x183].copy_from_slice(&[1, 0, b'T']);
        b[p + 0x1C..p + 0x20].copy_from_slice(&format.to_le_bytes());
        b[p + 0x24..p + 0x28].copy_from_slice(&w.to_le_bytes());
        b[p + 0x28..p + 0x2C].copy_from_slice(&h.to_le_bytes());
        b[p + 0x70..p + 0x78].copy_from_slice(&0x190u64.to_le_bytes()); // (its mips' table)
        b[0x190..0x198].copy_from_slice(&data.to_le_bytes());
        b
    }

    #[test]
    fn a_texture_that_says_a_size_past_its_file_is_refused() {
        // (a billion pixels a side, data past the end, data at u64::MAX: before, read as they said; now left out)
        assert!(bntx_textures(&bntx_of(0x2D06, 1_000_000_000, 4, 0x200, 0x200)).unwrap().is_empty());
        assert!(bntx_textures(&bntx_of(0x2D06, 64, 64, 0x200, 0x200)).unwrap().is_empty());
        assert!(bntx_textures(&bntx_of(0x2D06, 64, 64, u64::MAX, 0x200)).unwrap().is_empty());
        // (one that fits: 64 x 64 ASTC 4x4 is 4 KB of blocks, rounded to whole GOBs)
        let fits = bntx_of(0x2D06, 64, 64, 0x200, 0x200 + 64 * 1024);
        let t = bntx_textures(&fits).unwrap();
        assert_eq!((t[0].0.as_str(), t[0].1.width), ("T", 64));
        assert!(decode(&fits, &t[0].1).is_ok());
    }

    #[test]
    fn the_gob_layout_puts_each_block_once() {
        let t = Texture { format: 0x2D06, width: 64, height: 40, gobs: 2, data: 1000 };
        let (bw, bh) = t.blocks();
        let mut seen = std::collections::HashSet::new();
        for y in 0..bh {
            for x in 0..bw {
                assert!(seen.insert(t.block_at(x, y)));
            }
        }
    }

    #[test]
    fn runs_join_neighbours_and_a_mask_picks_blocks() {
        let t = Texture { format: 0x0100, width: 16, height: 8, gobs: 1, data: 0 };
        let im = Image::new(16, 8, 1, &[200]);
        let enc = |tx: &[[u8; 4]], s: bool| encode_for(&t, tx, s);
        let all = runs(&t, &im, None, None, &enc);
        assert_eq!(all.iter().map(|r| r.1.len()).sum::<usize>(), 8 * 8);
        let mut mask = Image::grey(16, 8);
        mask.data[5 * 16 + 9] = 1;
        let one = runs(&t, &im, None, Some(&mask), &enc);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].1.len(), 8);
        assert_eq!(one[0].0, t.block_at(2, 1));
    }
}

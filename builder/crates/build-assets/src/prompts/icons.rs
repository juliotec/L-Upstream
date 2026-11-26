//! A face's icon for each pad input as pictures: the glyph (the game's own for the Switch face, else Kenney's),
//! drawn large and fitted in a square, its silhouette and its holes (the letter in a button).

use super::faces::{self, PADS, SWITCH_ICON};
use crate::font::outline::Outline;
use crate::raster::{Filter, Image};
use build_core::{Error, Result};

/// The glyphs of a face's icons.
pub struct Icons {
    outlines: Vec<Option<Outline>>,
    upem: f64,
}

/// An icon's three masks, each `size` x `size`.
pub struct Masks {
    /// The icon, smooth.
    pub icon: Image,
    /// Its silhouette: inside its outer edge, the holes filled.
    pub shape: Image,
    /// Its holes (the letter in a button), off the icon's soft rim.
    pub holes: Image,
}

impl Icons {
    /// The icons of `face`: the game's font's for `switch` (`game`: the unwrapped font), else the face's Kenney font
    /// (`kenney`). A pad input the face has no glyph for (in its cmap) has none.
    pub fn new(face: &str, game: &[u8], kenney: Option<&[u8]>) -> Result<Icons> {
        fn parse<'b>(b: &'b [u8], what: &str) -> Result<ttf_parser::Face<'b>> {
            let f = ttf_parser::Face::parse(b, 0)
                .map_err(|e| Error::format(what, format!("not a font this reads ({e})")))?;
            // (an em of a few units makes a glyph thousands of pixels at the icons' scale)
            if f.units_per_em() < 64 {
                return Err(Error::format(what, format!("{} units an em: not a font this reads", f.units_per_em())));
            }
            Ok(f)
        }
        if face == "switch" {
            let f = parse(game, "the game's icon font")?;
            let outlines = SWITCH_ICON
                .iter()
                .map(|&c| char::from_u32(c).and_then(|c| f.glyph_index(c)).map(|g| Outline::of(&f, g)))
                .collect();
            return Ok(Icons { outlines, upem: f64::from(f.units_per_em()) });
        }
        let spec = faces::face(face).ok_or_else(|| Error::format("prompts", format!("no face {face}")))?;
        let bytes = kenney.ok_or_else(|| Error::format(spec.font, "Kenney's font is missing"))?;
        let f = parse(bytes, spec.font)?;
        // (a glyph counts when the font maps a code point to it, as the Python's reverse cmap did)
        let mut mapped = std::collections::HashSet::new();
        if let Some(cmap) = f.tables().cmap {
            for s in cmap.subtables {
                if s.is_unicode() {
                    s.codepoints(|c| {
                        if let Some(g) = s.glyph_index(c) {
                            mapped.insert(g.0);
                        }
                    });
                }
            }
        }
        let outlines = PADS
            .iter()
            .map(|pad| {
                let name = spec.icon(pad)?;
                let g = f.glyph_index_by_name(&name).filter(|g| mapped.contains(&g.0))?;
                Some(Outline::of(&f, g))
            })
            .collect();
        Ok(Icons { outlines, upem: f64::from(f.units_per_em()) })
    }

    /// Whether the face has an icon for pad input `pad` (a number of [`PADS`]).
    pub fn has(&self, pad: usize) -> bool {
        self.outlines[pad].is_some()
    }

    /// The icon of `pad` as masks of `size` x `size`, centred: the glyph drawn at `size` x 4 pixels an em, cut to its
    /// pixels, fitted (Lanczos) and its silhouette and holes found.
    pub fn masks(&self, pad: usize, size: usize) -> Masks {
        let empty =
            || Masks { icon: Image::grey(size, size), shape: Image::grey(size, size), holes: Image::grey(size, size) };
        let Some(o) = &self.outlines[pad] else { return empty() };
        let Some((x0, y0, x1, y1)) = o.bounds() else { return empty() };
        let scale = (size * 4) as f64 / self.upem;
        // (whole pixels from the pen's origin, as a font renderer places the glyph)
        let (px0, py0) = ((x0 * scale).floor(), (-y1 * scale).floor());
        let (w, h) = (((x1 * scale).ceil() - px0) as usize + 1, ((-y0 * scale).ceil() - py0) as usize + 1);
        // (a glyph of more than two ems is no icon: drawn, it would be a picture of any size)
        if w > 8 * size || h > 8 * size {
            return empty();
        }
        let drawn = o.rasterize(scale, px0 / scale, -py0 / scale, w, h);
        let Some((bx0, by0, bx1, by1)) = drawn.bbox() else { return empty() };
        let g = drawn.crop(bx0 as i64, by0 as i64, bx1 as i64, by1 as i64);
        let s = size as f64 / g.width.max(g.height) as f64;
        let fit = |v: usize| ((v as f64 * s).round_ties_even() as usize).max(1);
        let g = g.resize(fit(g.width), fit(g.height), Filter::Lanczos);
        let mut icon = Image::grey(size, size);
        icon.place(&g, ((size - g.width.min(size)) / 2) as i64, ((size - g.height.min(size)) / 2) as i64);
        // (what the corner reaches from a margin round the icon is its outside)
        let mut outside = Image::new(size + 4, size + 4, 1, &[255]);
        outside.place(&icon.threshold(127).invert(), 2, 2);
        outside.flood(0, 0, 128);
        let shape = outside.crop(2, 2, size as i64 + 2, size as i64 + 2).map(|v| if v == 128 { 0 } else { 255 });
        let inner = shape.rank(5, false); // (the soft rim of the icon is not a hole)
        let holes = inner.multiply(&icon.invert());
        Masks { icon, shape, holes }
    }
}

/// How much of `m` is lit, 0 to 1 (the mean of its values over 255).
pub fn coverage(m: &Image) -> f64 {
    m.data.iter().map(|&v| f64::from(v)).sum::<f64>() / 255.0 / m.data.len().max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use ttf_parser::OutlineBuilder;

    #[test]
    fn a_glyph_far_past_its_em_draws_no_picture_of_its_size() {
        let mut o = Outline::default();
        o.move_to(0.0, 0.0);
        o.line_to(1.0e9, 0.0);
        o.line_to(1.0e9, 1.0e9);
        o.close();
        let icons = Icons { outlines: vec![Some(o)], upem: 1000.0 };
        let started = std::time::Instant::now();
        let m = icons.masks(0, 64);
        assert!(m.icon.width == 64 && started.elapsed() < std::time::Duration::from_secs(5));
    }
}

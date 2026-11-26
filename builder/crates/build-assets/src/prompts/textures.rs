//! The textures with button icons (`UI/Layout2Archive/Common.*.bntx`): the ocarina's notes, the letters on black,
//! the controller in the pause menu's controls page and its leader lines, made for each face as layers (pictures of
//! a texture's, for a Switch button on a pad input or always), then encoded into the blocks `host/prompts.c` writes
//! over the unpacked archive for the controls in use.
//!
//! ```text
//! <face>/textures.bin: "HPTX", u32 count, then each: u8 Switch button (HOS_SW_*; 0xff: always), u8 pad input
//!     (HOS_PAD_*), u16 0, u32 runs, then the runs: u32 offset in Common.bntx, u32 length, the bytes
//! ```
//!
//! A layer's name: the texture's, then for a Switch button's picture on a pad input `.<pad input>` (the notes, the
//! letters; the leader lines `.<Switch button>.<pad input>`: only the line, drawn over the lines that stay), `.png`.
//! A PNG of that name in the face's folder of hand-made pictures takes the layer's place (`--dump` writes them all).

use super::faces::{self, DEFAULT, PADS, pad, sw};
use super::guides::{self, BARS, GUIDES, HAND_GUIDES};
use super::icons::{Icons, coverage};
use crate::raster::{Filter, Image};
use crate::texture::{self, Run, Texture};
use build_core::{Error, Result};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

/// The texture archive, under the romfs.
pub const TEXTURES: &str = "UI/Layout2Archive/Common.Product.100.Nin_Ounce_NVN2.bntx.zs";
/// The ocarina's notes (white icons, dark letters), by Switch button.
const NOTES: [(&str, &str); 6] = [
    ("OcarinaButtonA_00", "a"),
    ("OcarinaButtonB_00", "b"),
    ("OcarinaButtonX_00", "x"),
    ("OcarinaButtonY_00", "y"),
    ("OcarinaButtonL_00", "l"),
    ("OcarinaButtonR_00", "r"),
];
/// The white letters on black (BC4).
const LETTERS: [(&str, &str); 4] =
    [("KeyTexFontA_00", "a"), ("KeyTexFontB_00", "b"), ("KeyTexFontX_00", "x"), ("KeyTexFontY_00", "y")];
/// The Pro Controller in the controls page: the picture and the page's copy of it (tinted), and its shadow.
const PICTURES: [&str; 2] = ["Controller_Pro", "Controller_Pro1"];
const PICTURE_TINT: &str = "Controller_Pro1";
const PICTURE_SHADOW: &str = "ControllerSh_Pro1";
/// The face buttons' discs in the game's picture (centre, radius), by the pad input there.
const PICTURE_BUTTONS: [(&str, (i64, i64, i64)); 4] =
    [("east", (869, 227, 31)), ("south", (800, 286, 31)), ("north", (800, 168, 31)), ("west", (732, 227, 31))];
/// The textures drawn on, as the game has them: (name, format, width, height). The pictures' places (the buttons'
/// discs, the guides' lines) are this size's; a texture of another (another game version, a mod's in its place) is
/// not drawn on ([`unknown`]).
pub const KNOWN: [(&str, u32, usize, usize); 18] = [
    ("KeyTexFontA_00", 0x1d01, 64, 64),
    ("KeyTexFontB_00", 0x1d01, 64, 64),
    ("KeyTexFontX_00", 0x1d01, 64, 64),
    ("KeyTexFontY_00", 0x1d01, 64, 64),
    ("Controller_Pro", 0x2d06, 1160, 680),
    ("Controller_Pro1", 0x2d06, 1160, 680),
    ("ControllerGuide_Pro_00", 0x1d01, 1555, 1511),
    ("ControllerGuide_Pro_01", 0x1d01, 1555, 1511),
    ("ControllerGuide_Pro_02", 0x1d01, 1555, 1511),
    ("ControllerGuide_Pro_03", 0x1d01, 1555, 1511),
    ("ControllerGuide_Pro_04", 0x1d01, 1555, 1511),
    ("ControllerSh_Pro1", 0x1d01, 440, 580),
    ("OcarinaButtonA_00", 0x2d06, 93, 93),
    ("OcarinaButtonB_00", 0x2d06, 93, 93),
    ("OcarinaButtonL_00", 0x2d06, 93, 93),
    ("OcarinaButtonR_00", 0x2d06, 93, 93),
    ("OcarinaButtonX_00", 0x2d06, 93, 93),
    ("OcarinaButtonY_00", 0x2d06, 93, 93),
];

/// The first texture of [`KNOWN`] the archive has not as it is known (missing, or of another format or size), said;
/// `None` when every one is.
pub fn unknown(textures: &HashMap<String, Texture>) -> Option<String> {
    KNOWN.iter().find_map(|&(name, format, w, h)| match textures.get(name) {
        None => Some(format!("no {name}")),
        Some(t) if (t.format, t.width, t.height) != (format, w, h) => {
            Some(format!("{name} is {} x {} (format {:#x}), not {w} x {h} ({format:#x})", t.width, t.height, t.format))
        }
        Some(_) => None,
    })
}

/// A Switch button number that means "always".
const ALWAYS: u8 = 0xFF;

/// A picture of a texture's: written whole, or its box of it, or drawn over the layer `over` where it has any of
/// itself; for a Switch button ([`ALWAYS`]: always) on a pad input.
pub struct Layer {
    /// Its name (`<texture>[.<…>].png`).
    pub name: String,
    /// Its texture.
    pub texture: &'static str,
    /// The Switch button.
    pub sw: u8,
    /// The pad input.
    pub pad: u8,
    /// The picture (the texture's size: RGBA for ASTC, grey for BC4).
    pub im: Rc<Image>,
    /// The box written (pixels), if not all.
    pub bx: Option<(i64, i64, i64, i64)>,
    /// The layer it is drawn over.
    pub over: Option<String>,
}

/// What a face's layers are made from.
pub struct Inputs<'a> {
    /// The unpacked texture archive.
    pub bntx: &'a [u8],
    /// Its textures.
    pub textures: &'a HashMap<String, Texture>,
    /// The game's icon font (unwrapped).
    pub game_font: &'a [u8],
    /// The face's Kenney font.
    pub kenney: Option<&'a [u8]>,
    /// The folder of the face's hand-made pictures (`<faces>/<face>/`), if any.
    pub hand: Option<&'a Path>,
}

impl Inputs<'_> {
    fn tex(&self, name: &str) -> Result<&Texture> {
        self.textures.get(name).ok_or_else(|| Error::format("Common.bntx", format!("no texture {name}")))
    }

    fn decode(&self, name: &str) -> Result<Image> {
        texture::decode(self.bntx, self.tex(name)?)
    }

    fn hand_file(&self, name: &str) -> Option<std::path::PathBuf> {
        self.hand.map(|d| d.join(name)).filter(|p| p.is_file())
    }

    fn hand_image(&self, name: &str, channels: usize) -> Result<Option<Image>> {
        match self.hand_file(name) {
            None => Ok(None),
            Some(p) => {
                let bytes = std::fs::read(&p).map_err(|e| Error::io(&p, e))?;
                Image::from_png(&bytes, channels, &p.display().to_string()).map(Some)
            }
        }
    }
}

fn layer(name: String, texture: &'static str, sw: u8, pad: u8, im: Image) -> Layer {
    Layer { name, texture, sw, pad, im: Rc::new(im), bx: None, over: None }
}

/// A face's layers, in `textures.bin`'s order, each a hand-made one when the face's folder has it. `log` hears of a
/// game line not found.
pub fn face_layers(face: &str, inp: &Inputs<'_>, log: &mut dyn FnMut(String)) -> Result<Vec<Layer>> {
    let icons = Icons::new(face, inp.game_font, inp.kenney)?;
    let mut layers = Vec::new();
    // (the Switch face's default controls: the game's own textures)
    let keep = |s: &str, p: usize| icons.has(p) && !(face == "switch" && DEFAULT[sw(s)] == PADS[p]);
    for (name, s) in NOTES {
        let t = inp.tex(name)?;
        let (w, h) = (t.width, t.height);
        for (p, pad_name) in PADS.iter().enumerate() {
            if !keep(s, p) {
                continue;
            }
            let m = icons.masks(p, (w.min(h) as f64 * 0.9) as usize);
            let mut im = Image::rgba(w, h);
            let (ox, oy) = (((w - m.icon.width) / 2) as i64, ((h - m.icon.height) / 2) as i64);
            im.paste_colour(&[255, 255, 255, 255], &m.shape, ox, oy);
            im.paste_colour(&[0, 0, 0, 255], &m.holes, ox, oy);
            layers.push(layer(format!("{name}.{pad_name}.png"), name, sw(s) as u8, p as u8, im));
        }
    }
    for (name, s) in LETTERS {
        let t = inp.tex(name)?;
        let (w, h) = (t.width, t.height);
        for (p, pad_name) in PADS.iter().enumerate() {
            if !keep(s, p) {
                continue;
            }
            let m = icons.masks(p, (w.min(h) as f64 * 0.9) as usize);
            let letter = if m.holes.bbox().is_some() { m.holes } else { m.icon }; // (no letter in it: its shape)
            let mut im = Image::grey(w, h);
            im.place(&letter, ((w - letter.width) / 2) as i64, ((h - letter.height) / 2) as i64);
            layers.push(layer(format!("{name}.{pad_name}.png"), name, sw(s) as u8, p as u8, im));
        }
    }
    let mut own = None;
    if face != "switch" {
        own = inp.hand_image(&format!("{}.png", PICTURES[0]), 4)?;
        let lines: Vec<&str> = HAND_GUIDES
            .iter()
            .copied()
            .filter(|n| !GUIDES.contains(n) && inp.hand_file(&format!("{n}.png")).is_some())
            .collect();
        if let Some(first) = lines.first()
            && own.is_none()
        {
            // (lines drawn for a picture that isn't there: they would point at the drawn one's places)
            return Err(Error::player(
                "prompt_picture_missing",
                format!("The {face} face gives {first}.png but not the {}.png it was drawn for.", PICTURES[0]),
            ));
        }
    }
    match own {
        Some(im) => layers.extend(picture_layers(inp, im)?),
        None if face != "switch" => {
            // (the game's Pro Controller, its face buttons given the face's labels)
            for name in PICTURES {
                let mut im = inp.decode(name)?;
                for (p, (cx, cy, r)) in PICTURE_BUTTONS {
                    let fill = im.px((cx + r - 6) as usize, cy as usize).to_vec();
                    im.fill_ellipse((cx - r, cy - r, cx + r, cy + r), &fill);
                    let size = (2.0 * r as f64 * 0.92) as usize;
                    let m = icons.masks(pad(p), size);
                    let mark = if coverage(&m.shape) > 0.7 {
                        m.holes
                    } else {
                        icons.masks(pad(p), (size as f64 * 0.7) as usize).icon
                    };
                    im.paste_colour(
                        &[205, 205, 205, 255],
                        &mark,
                        cx - (mark.width / 2) as i64,
                        cy - (mark.height / 2) as i64,
                    );
                }
                let r = PICTURE_BUTTONS.iter().map(|b| b.1.2).max().unwrap_or(0);
                let xs = PICTURE_BUTTONS.iter().map(|b| b.1.0);
                let ys = PICTURE_BUTTONS.iter().map(|b| b.1.1);
                let bx = (
                    xs.clone().min().unwrap_or(0) - r,
                    ys.clone().min().unwrap_or(0) - r,
                    xs.max().unwrap_or(0) + r + 1,
                    ys.max().unwrap_or(0) + r + 1,
                );
                let mut l = layer(format!("{name}.png"), name, ALWAYS, 0, im);
                l.bx = Some(bx);
                layers.push(l);
            }
        }
        None => {}
    }
    layers.extend(guide_layers(face, inp, log)?);
    hand_layers(inp, &mut layers)?;
    Ok(layers)
}

/// The layers with each hand-made picture of the face's folder in place of the made one (of the same size); not
/// `ControllerGuide_Pro_02` to `04`, already put over the game's bars.
fn hand_layers(inp: &Inputs<'_>, layers: &mut [Layer]) -> Result<()> {
    for l in layers.iter_mut() {
        if HAND_GUIDES.contains(&l.texture) && !GUIDES.contains(&l.texture) {
            continue;
        }
        if let Some(im) = inp.hand_image(&l.name, l.im.channels)? {
            if im.size() != l.im.size() {
                return Err(wrong_size(&l.name, &im, &l.im));
            }
            l.im = Rc::new(im);
        }
    }
    Ok(())
}

fn wrong_size(name: &str, im: &Image, want: &Image) -> Error {
    Error::player(
        "prompt_picture_size",
        format!("The hand-made {name} is {}x{}; its texture is {}x{}.", im.width, im.height, want.width, want.height),
    )
}

/// The layers putting a face's controller picture (1160x680 RGBA, drawn in the greys of `Controller_Pro`) in place of
/// the Pro Controller: the picture, the page's tinted copy (each grey the colour the game's own pair gives it) and
/// the shadow (the outline's left part squeezed into `ControllerSh_Pro1` and blurred).
fn picture_layers(inp: &Inputs<'_>, im: Image) -> Result<Vec<Layer>> {
    let (plain, tinted) = (inp.decode(PICTURES[0])?, inp.decode(PICTURE_TINT)?);
    // (the greys the game drew enough of, each with the median of the colours it has in the tinted copy)
    let mut by_grey: std::collections::BTreeMap<i64, Vec<[f64; 3]>> = std::collections::BTreeMap::new();
    for (p, t) in plain.data.as_chunks::<4>().0.iter().zip(tinted.data.as_chunks::<4>().0) {
        if p[3] > 250 && t[3] > 250 {
            let grey = ((f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0).round_ties_even() as i64;
            by_grey.entry(grey).or_default().push([f64::from(t[0]), f64::from(t[1]), f64::from(t[2])]);
        }
    }
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let n = v.len();
        if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
    };
    let table: Vec<(f64, [f64; 3])> = by_grey
        .into_iter()
        .filter(|(_, c)| c.len() >= 20)
        .map(|(g, c)| (g as f64, [0, 1, 2].map(|k| median(c.iter().map(|x| x[k]).collect()))))
        .collect();
    if table.is_empty() {
        return Err(Error::format(
            "Common.bntx",
            "no grey of the game's Controller_Pro has its colour in Controller_Pro1",
        ));
    }
    let interp = |x: f64, k: usize| {
        if x <= table[0].0 {
            return table[0].1[k];
        }
        let last = table.len() - 1;
        if x >= table[last].0 {
            return table[last].1[k];
        }
        let i = table.partition_point(|e| e.0 <= x) - 1;
        let (x0, y0, x1, y1) = (table[i].0, table[i].1[k], table[i + 1].0, table[i + 1].1[k]);
        y0 + (x - x0) * (y1 - y0) / (x1 - x0)
    };
    let mut tint = im.clone();
    for px in tint.data.as_chunks_mut::<4>().0 {
        let level = (f64::from(px[0]) + f64::from(px[1]) + f64::from(px[2])) / 3.0;
        for (k, v) in px.iter_mut().take(3).enumerate() {
            *v = interp(level, k).round_ties_even().clamp(0.0, 255.0) as u8;
        }
    }
    let sh = inp.tex(PICTURE_SHADOW)?;
    let shadow = im
        .channel(3)
        .crop(40, 0, 480, 680)
        .resize(sh.width, sh.height, Filter::Bicubic)
        .gaussian_blur(24.0)
        .map(|v| (f64::from(v) * 0.7).round_ties_even() as u8);
    Ok(vec![
        layer(format!("{}.png", PICTURES[0]), PICTURES[0], ALWAYS, 0, im),
        layer(format!("{PICTURE_TINT}.png"), PICTURE_TINT, ALWAYS, 0, tint),
        layer(format!("{PICTURE_SHADOW}.png"), PICTURE_SHADOW, ALWAYS, 0, shadow),
    ])
}

/// The layers of the page's leader lines: each texture's lines that stay (always), then each Switch button's line
/// for each pad input drawn over them; then the hand-made `ControllerGuide_Pro_02` to `04` over the game's bars.
fn guide_layers(face: &str, inp: &Inputs<'_>, log: &mut dyn FnMut(String)) -> Result<Vec<Layer>> {
    let Some(parts) = faces::parts(face) else { return Ok(Vec::new()) };
    let game = [inp.decode(GUIDES[0])?, inp.decode(GUIDES[1])?];
    let (keep, mut moving) = guides::guide_layers(&game, face, &parts, log);
    if GUIDES.iter().all(|n| inp.hand_file(&format!("{n}.png")).is_some()) {
        moving = vec![Vec::new(), Vec::new()]; // (every line of these two is given by hand: nothing drawn over them)
    }
    let mut layers = Vec::new();
    for (tx, name) in GUIDES.iter().enumerate() {
        // (every place a line is or may be)
        let mut all = game[tx].lighter(&keep[tx]);
        for (_, lines) in &moving[tx] {
            for (_, im) in lines {
                all = all.lighter(im);
            }
        }
        let mut l = Layer {
            name: format!("{name}.png"),
            texture: name,
            sw: ALWAYS,
            pad: 0,
            im: keep[tx].clone(),
            bx: all.bbox().map(|(a, b, c, d)| (a as i64, b as i64, c as i64, d as i64)),
            over: None,
        };
        if l.bx.is_none() {
            l.bx = Some((0, 0, 0, 0));
        }
        layers.push(l);
        for (end, lines) in &moving[tx] {
            for (p, im) in lines {
                layers.push(Layer {
                    name: format!("{name}.{end}.{p}.png"),
                    texture: name,
                    sw: sw(end) as u8,
                    pad: pad(p) as u8,
                    im: im.clone(),
                    bx: None,
                    over: Some(format!("{name}.png")),
                });
            }
        }
    }
    for name in HAND_GUIDES.iter().filter(|n| !GUIDES.contains(n)) {
        let Some(im) = inp.hand_image(&format!("{name}.png"), 1)? else { continue };
        let own = inp.decode(name)?;
        if im.size() != own.size() {
            return Err(wrong_size(&format!("{name}.png"), &im, &own));
        }
        // (the game's own lines go with ours: ours cover them; none: no lines)
        let Some(bx) = own.lighter(&im).bbox() else { continue };
        let mut bars = Image::grey(own.width, own.height);
        bars.place(&own.crop(0, 0, BARS.0, own.height as i64), 0, 0);
        bars.place(&own.crop(BARS.1, 0, own.width as i64, own.height as i64), BARS.1, 0);
        let mut l = layer(format!("{name}.png"), name, ALWAYS, 0, bars.lighter(&im));
        l.bx = Some((bx.0 as i64, bx.1 as i64, bx.2 as i64, bx.3 as i64));
        layers.push(l);
    }
    Ok(layers)
}

/// A block's encoder: its texture, its 16 texels, whether the texture is sRGB.
pub type BlockEncoder<'a> = dyn Fn(&Texture, &[[u8; 4]], bool) -> Vec<u8> + 'a;

/// One of `textures.bin`'s entries: (Switch button, pad input, runs).
pub type Entry = (u8, u8, Vec<Run>);

/// `textures.bin`'s entries for `layers`: (Switch button, pad input, runs). `encode` encodes a block of a texture.
pub fn face_textures(layers: &[Layer], inp: &Inputs<'_>, encode: &BlockEncoder<'_>) -> Result<Vec<Entry>> {
    let mut made: HashMap<&str, Rc<Image>> = HashMap::new();
    let mut out = Vec::with_capacity(layers.len());
    for l in layers {
        let t = inp.tex(l.texture)?;
        let enc = |texels: &[[u8; 4]], srgb: bool| encode(t, texels, srgb);
        made.insert(&l.name, l.im.clone());
        let runs = if let Some(over) = &l.over {
            let under = made.get(over.as_str()).ok_or_else(|| Error::format("prompts", format!("no layer {over}")))?;
            texture::runs(t, &under.lighter(&l.im), None, Some(&l.im), &enc)
        } else if let Some((x0, y0, x1, y1)) = l.bx {
            // (a hand-made picture may reach further)
            let bx = match l.im.bbox() {
                Some((a, b, c, d)) => (x0.min(a as i64), y0.min(b as i64), x1.max(c as i64), y1.max(d as i64)),
                None => (x0, y0, x1, y1),
            };
            texture::runs(t, &l.im, Some(bx), None, &enc)
        } else {
            texture::runs(t, &l.im, None, None, &enc)
        };
        let mut runs = runs;
        crate::prompts::layout::keep_within(&mut runs, inp.bntx.len());
        out.push((l.sw, l.pad, runs));
    }
    Ok(out)
}

/// `textures.bin`'s bytes.
pub fn textures_bin(entries: &[Entry]) -> Vec<u8> {
    let mut out = b"HPTX".to_vec();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (s, p, runs) in entries {
        out.extend_from_slice(&[*s, *p, 0, 0]);
        super::layout::put_runs(&mut out, runs);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_of_another_size_are_not_drawn_on() {
        let mut t: HashMap<String, Texture> = KNOWN
            .iter()
            .map(|&(n, format, width, height)| (n.to_string(), Texture { format, width, height, gobs: 1, data: 0 }))
            .collect();
        assert_eq!(unknown(&t), None);
        t.get_mut("Controller_Pro").unwrap().width = 800; // (smaller than the buttons' discs at x 869)
        assert!(unknown(&t).unwrap().contains("Controller_Pro is 800 x 680"));
        t.remove("KeyTexFontA_00");
        assert_eq!(unknown(&t).unwrap(), "no KeyTexFontA_00");
    }
}

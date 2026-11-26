//! The font with every face's icons: the game's button-icon font (`Font2/Scalable/External.*.bfotf`), each face's
//! icons added from Kenney's fonts, and the keyboard face's keys.
//!
//! Every text in the game draws its button icons from this font: A B X Y at U+E040..E043, L R ZL ZR at E044..E047,
//! Plus and Minus at E04B and E04C, the stick presses at E056 and E057, the d-pad at E060..E063. The font made has,
//! for each face, one icon per pad input at [`PAD_BASE`] + [`FACE_CODES`] x face + pad input and the face's d-pad
//! icons at + [`STATIC_AT`]..; each Switch button's icon on each pad input at [`BIND_BASE`]..; and each of the Switch
//! buttons' and d-pad icons' code points in a cmap segment of its own: `host/prompts.c` points each at the face's icon
//! of the pad input bound to it, and again when the face or the controls change, so the icons follow the controls.
//! As built, the font's code points are the game's.

use super::faces::{self, PADS, STATIC, SWITCH, SWITCH_ICON};
use super::keys::{self, Stick};
use crate::font::FontEdit;
use crate::font::outline::{Contour, Outline, Seg};
use build_core::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// The first face's first pad icon.
pub const PAD_BASE: u32 = 0xF700;
/// The code points of each face: its pad icons, then at [`STATIC_AT`] its d-pad icons.
pub const FACE_CODES: u32 = 32;
/// Where a face's d-pad icons start, after its pad icons.
pub const STATIC_AT: u32 = 24;
/// A Switch button's icon in a text once the face or the controls changed while the game runs: `BIND_BASE + (face x
/// 16 + Switch button) x 17 + pad input`, the face's icon of that pad input (a code of its own for each: the game
/// keeps a code's glyph drawn, so a new icon is drawn anew).
pub const BIND_BASE: u32 = 0xF000;
/// How much wider than the game's icon a wide Kenney icon may be (the Menu and View pills).
const WIDE: f64 = 1.6;
/// How much wider than the game's icon a word key may be.
const MOST: f64 = 2.2;
/// A quarter circle's control points, as a fraction of its radius.
const K: f64 = 0.5523;

/// The fonts the font is made from.
pub struct Fonts<'a> {
    /// The game's icon font (unwrapped OpenType).
    pub game: &'a [u8],
    /// The game's bold font, for the keyboard's word keys (without it: Kenney's keys).
    pub label: Option<&'a [u8]>,
    /// Kenney's fonts by file name (those of the faces, and the keyboard's).
    pub kenney: &'a BTreeMap<String, Vec<u8>>,
}

/// What [`faces_font`] makes.
pub struct Made {
    /// The font (unwrapped OpenType).
    pub font: Vec<u8>,
    /// Each face's icons' advances: per pad input; the keyboard's per key code (256, 0 none).
    pub widths: Vec<Vec<u16>>,
}

fn round_even(v: f64) -> i32 {
    v.round_ties_even() as i32
}

fn bounds_of(o: &Outline) -> (f64, f64, f64, f64) {
    o.bounds().unwrap_or_default()
}

/// A Kenney font, with what is drawn from it.
struct Kenney<'a> {
    face: ttf_parser::Face<'a>,
}

impl Kenney<'_> {
    fn glyph(&self, name: &str) -> Option<Outline> {
        self.face.glyph_index_by_name(name).map(|g| Outline::of(&self.face, g))
    }
}

/// The glyph `kn` of `k` scaled into the box of the game's glyph `target` (as a square round its centre); a wide one
/// as high as the box, up to [`WIDE`] times as wide, the glyph widened. (The outline, its advance.)
fn draw(game: &FontEdit<'_>, target: u16, k: &Kenney<'_>, kn: &str, scale: f64) -> Option<(Outline, i32)> {
    let glyph = k.glyph(kn)?;
    let gb = bounds_of(&game.outline(target));
    let kb = glyph.bounds()?;
    let side = (gb.2 - gb.0).max(gb.3 - gb.1);
    let s = scale * (side * WIDE / (kb.2 - kb.0)).min(side / (kb.3 - kb.1));
    let over = (s * (kb.2 - kb.0) - side).max(0.0);
    let dx = (gb.0 + gb.2 + over) / 2.0 - s * (kb.0 + kb.2) / 2.0;
    let dy = (gb.1 + gb.3) / 2.0 - s * (kb.1 + kb.3) / 2.0;
    Some((glyph.transformed(s, dx, dy), i32::from(game.advance(target)) + round_even(over)))
}

/// A rounded rectangle's contour (counter-clockwise, y up).
fn rrect(x0: f64, y0: f64, x1: f64, y1: f64, r: f64) -> Contour {
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let k = K * r;
    Contour {
        start: (x0 + r, y0),
        segs: vec![
            Seg::Line((x1 - r, y0)),
            Seg::Cubic((x1 - r + k, y0), (x1, y0 + r - k), (x1, y0 + r)),
            Seg::Line((x1, y1 - r)),
            Seg::Cubic((x1, y1 - r + k), (x1 - r + k, y1), (x1 - r, y1)),
            Seg::Line((x0 + r, y1)),
            Seg::Cubic((x0 + r - k, y1), (x0, y1 - r + k), (x0, y1 - r)),
            Seg::Line((x0, y0 + r)),
            Seg::Cubic((x0, y0 + r - k), (x0 + r - k, y0), (x0 + r, y0)),
        ],
    }
}

/// A mouse button's area: x `a..b`, from `y0` up to `top`, then a quarter ellipse up to (the inner side, `y1`) on the
/// outer side (left: at `a`), `ry` high and `b - a` wide.
fn button(a: f64, b: f64, y0: f64, top: f64, y1: f64, ry: f64, left: bool) -> Contour {
    let rx = b - a;
    let mut segs = vec![Seg::Line((b, y0))];
    if left {
        segs.push(Seg::Line((b, y1)));
        segs.push(Seg::Cubic((b - K * rx, y1), (a, top + K * ry), (a, top)));
    } else {
        segs.push(Seg::Line((b, top)));
        segs.push(Seg::Cubic((b, top + K * ry), (a + K * rx, y1), (a, y1)));
    }
    Contour { start: (a, y0), segs }
}

/// `c` added to `o`, turned to go counter-clockwise (y up) for `sign` 1, clockwise for -1.
fn contour(o: &mut Outline, sign: f64, c: Contour) {
    let ccw = Outline::contour_area(&c) > 0.0;
    o.contours.push(if ccw == (sign > 0.0) { c } else { c.reversed() });
}

/// A box's side (the larger of its width and height) and centre.
fn square_of(b: (f64, f64, f64, f64)) -> (f64, f64, f64) {
    ((b.2 - b.0).max(b.3 - b.1), (b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0)
}

/// The game's bold font, for the word keys.
struct Label<'a> {
    face: ttf_parser::Face<'a>,
}

impl Label<'_> {
    fn glyph(&self, c: char) -> Option<ttf_parser::GlyphId> {
        self.face.glyph_index(c)
    }

    /// Whether its letters' outer contours go counter-clockwise (its `I`'s).
    fn ccw(&self) -> bool {
        self.glyph('I')
            .map(|g| Outline::of(&self.face, g))
            .and_then(|o| o.contours.first().map(Outline::contour_area))
            .is_some_and(|a| a > 0.0)
    }
}

/// A key as high as the game's icon (`b`: that glyph's bounds) and as wide as the word needs, the word cut out of it:
/// the outline, and how much wider than the icon it is.
fn word_key(label: &str, b: (f64, f64, f64, f64), l: &Label<'_>) -> Option<(Outline, f64)> {
    let (side, cx, cy) = square_of(b);
    let glyphs: Vec<ttf_parser::GlyphId> = label.chars().map(|c| l.glyph(c)).collect::<Option<_>>()?;
    let width = |g: ttf_parser::GlyphId| f64::from(l.face.glyph_hor_advance(g).unwrap_or(0));
    let adv: f64 = glyphs.iter().map(|&g| width(g)).sum();
    let caph = Outline::of(&l.face, l.glyph('H')?).bounds()?.3;
    let mut s = side * 0.48 / caph; // (the capitals about half the key's height)
    let pad = side * 0.20;
    let mut w = side.max(adv * s + 2.0 * pad);
    if w > side * MOST {
        s *= (side * MOST - 2.0 * pad) / (adv * s);
        w = side * MOST;
    }
    let (x0, y0) = (cx - side / 2.0, cy - side / 2.0);
    let mut o = Outline::default();
    // (the letters' outer contours go the other way)
    contour(&mut o, if l.ccw() { -1.0 } else { 1.0 }, rrect(x0, y0, x0 + w, y0 + side, side * 0.2));
    let (mut x, ty) = (x0 + (w - adv * s) / 2.0, cy - caph * s / 2.0);
    for g in glyphs {
        o.extend(Outline::of(&l.face, g).transformed(s, x, ty));
        x += width(g) * s;
    }
    Some((o, w - side))
}

/// A key (as the others) with a mouse cut out of it, the button `which` (left, right; middle: the wheel) left white
/// in the mouse.
fn mouse(which: &str, b: (f64, f64, f64, f64)) -> Outline {
    let sign = 1.0;
    let (side, cx, cy) = square_of(b);
    let (x0, y0) = (cx - side / 2.0, cy - side / 2.0);
    let mut o = Outline::default();
    contour(&mut o, sign, rrect(x0, y0, x0 + side, y0 + side, side * 0.2)); // the key
    let (mw, mh) = (side * 0.52, side * 0.76);
    let (mx0, my0, mx1, my1) = (cx - mw / 2.0, cy - mh / 2.0, cx + mw / 2.0, cy + mh / 2.0);
    contour(&mut o, -sign, rrect(mx0, my0, mx1, my1, mw / 2.0)); // the mouse, cut out
    let g = side * 0.06; // (the dark line round the white button, and between the buttons)
    let (ix0, ix1, iy1, ri) = (mx0 + g, mx1 - g, my1 - g, mw / 2.0 - g);
    let by = cy + side * 0.02; // (the buttons' lower edge)
    let top = iy1 - ri;
    if which == "middle" {
        let ww = side * 0.13;
        contour(&mut o, sign, rrect(cx - ww / 2.0, by, cx + ww / 2.0, iy1 - side * 0.02, ww / 2.0));
    } else {
        let left = which == "left";
        let (a, b) = if left { (ix0, cx - g / 2.0) } else { (cx + g / 2.0, ix1) };
        contour(&mut o, sign, button(a, b, by, top, iy1, ri, left));
    }
    o
}

/// The font of `faces` (their names, in `HOS_PROMPT_*` order: the Switch, the faces, the keyboard).
pub fn faces_font(fonts: &Fonts<'_>, names: &[&str]) -> Result<Made> {
    let mut game = FontEdit::new(fonts.game)?;
    let cmap = game.cmap();
    let at =
        |c: u32| cmap.get(&c).copied().ok_or_else(|| Error::format("the game's icon font", format!("no U+{c:04X}")));
    let parse = |name: &str| -> Result<ttf_parser::Face<'_>> {
        let bytes = fonts.kenney.get(name).ok_or_else(|| Error::format(name, "Kenney's font is missing"))?;
        ttf_parser::Face::parse(bytes, 0).map_err(|e| Error::format(name, format!("not a font this reads ({e})")))
    };
    assert!(BIND_BASE + (names.len() * SWITCH.len() * PADS.len()) as u32 <= PAD_BASE);
    assert!(PAD_BASE + FACE_CODES * names.len() as u32 <= keys::KEY_BASE);
    let mut placed: BTreeMap<u32, u16> = BTreeMap::new();
    let mut widths = Vec::new();
    for (n, &name) in names.iter().enumerate() {
        let base = PAD_BASE + FACE_CODES * n as u32;
        if name == "switch" || name == "keyboard" {
            for (i, &icon) in SWITCH_ICON.iter().enumerate() {
                placed.insert(base + i as u32, at(icon)?);
            }
            for (k, &(c, _)) in STATIC.iter().enumerate() {
                if let Some(&g) = cmap.get(&c) {
                    placed.insert(base + STATIC_AT + k as u32, g);
                }
            }
        } else {
            let f = faces::face(name).ok_or_else(|| Error::format("prompts", format!("no face {name}")))?;
            let k = Kenney { face: parse(f.font)? };
            for (i, pad) in PADS.iter().enumerate() {
                let target = at(SWITCH_ICON[i])?;
                let g = match f.icon(pad).and_then(|kn| draw(&game, target, &k, &kn, 1.0)) {
                    Some((o, w)) => game.add(&format!("pad_{name}_{pad}"), &o, w),
                    None => target, // (no such button on this face, the PlayStation's PS button: the game's own icon)
                };
                placed.insert(base + i as u32, g);
            }
            for (kx, &(c, sn)) in STATIC.iter().enumerate() {
                let Some(&target) = cmap.get(&c) else { continue };
                let g = match draw(&game, target, &k, &format!("{}{sn}", f.prefix), 1.0) {
                    Some((o, w)) => game.add(&format!("static_{name}_{c:04x}"), &o, w),
                    None => target,
                };
                placed.insert(base + STATIC_AT + kx as u32, g);
            }
        }
        let mut face_widths: Vec<u16> = (0..PADS.len()).map(|i| game.advance(placed[&(base + i as u32)])).collect();
        if name == "keyboard" {
            let k = Kenney { face: parse(keys::FONT)? };
            let label = fonts.label.and_then(|b| ttf_parser::Face::parse(b, 0).ok()).map(|face| Label { face });
            let (keys_placed, key_widths) = keyboard_glyphs(&mut game, &cmap, label.as_ref(), &k)?;
            face_widths = key_widths;
            placed.extend(keys_placed);
            for i in 0..keys::SLOTS {
                placed.insert(keys::SLOT_BASE + i, at(SWITCH[0])?);
            }
        }
        widths.push(face_widths);
        for b in 0..SWITCH.len() {
            for i in 0..PADS.len() {
                let g = placed[&(base + i as u32)];
                placed.insert(BIND_BASE + ((n * SWITCH.len() + b) * PADS.len() + i) as u32, g);
            }
        }
    }
    let mut mapping: BTreeMap<u32, u16> = cmap.iter().filter(|(c, _)| **c <= 0xFFFF).map(|(c, g)| (*c, *g)).collect();
    mapping.extend(placed);
    let mut singles: BTreeSet<u32> = SWITCH.iter().copied().chain(STATIC.iter().map(|s| s.0)).collect();
    if names.contains(&"keyboard") {
        singles.extend(keys::STICKS.iter().map(|s| s.0));
        singles.extend((0..keys::SLOTS).map(|i| keys::SLOT_BASE + i));
    }
    Ok(Made { font: game.write(&mapping, &singles)?, widths })
}

/// The keyboard face's glyphs: every key at [`keys::KEY_BASE`] + its code and the sticks at [`keys::STICK_KEYS`]..,
/// and each key's advance by its code (256 of them, 0 none).
fn keyboard_glyphs(
    game: &mut FontEdit<'_>,
    cmap: &BTreeMap<u32, u16>,
    label: Option<&Label<'_>>,
    k: &Kenney<'_>,
) -> Result<(BTreeMap<u32, u16>, Vec<u16>)> {
    let target = *cmap.get(&0xE040).ok_or_else(|| Error::format("the game's icon font", "no U+E040"))?; // (each key in the box of the game's A)
    let b = bounds_of(&game.outline(target));
    let width0 = i32::from(game.advance(target));
    let mut placed = BTreeMap::new();
    let mut widths = vec![0u16; 256];
    for code in 1..256u32 {
        let name = format!("key_{code:02x}");
        let g = if let Some(which) = keys::mouse(code) {
            game.add(&name, &mouse(which, b), width0)
        } else if let Some((o, over)) = label.zip(keys::label(code)).and_then(|(l, text)| word_key(&text, b, l)) {
            game.add(&name, &o, width0 + round_even(over))
        } else if keys::key(code).is_some()
            && let Some((o, w)) = draw(game, target, k, &keys::kenney_name(code), 1.0)
        {
            game.add(&name, &o, w)
        } else {
            continue;
        };
        placed.insert(keys::KEY_BASE + code, g);
        widths[code as usize] = game.advance(g);
    }
    let mut made: BTreeMap<String, u16> = BTreeMap::new();
    for (i, &(code, stick, ways)) in keys::STICKS.iter().enumerate() {
        let own = *cmap.get(&code).ok_or_else(|| Error::format("the game's icon font", format!("no U+{code:04X}")))?;
        placed.insert(keys::STICK_GAME + i as u32, own);
        let g = if stick == Stick::Cam {
            // (the mouse moving, those ways; the arrows round it take room: a little larger than a key)
            let kn = if ways.len() == 4 {
                "mouse_move"
            } else if ways.starts_with(['l', 'r']) {
                "mouse_horizontal"
            } else {
                "mouse_vertical"
            };
            match made.get(kn) {
                Some(&g) => g,
                None => match draw(game, target, k, kn, 1.2) {
                    Some((o, w)) => {
                        let g = game.add(&format!("stick_{kn}"), &o, w);
                        made.insert(kn.into(), g);
                        g
                    }
                    None => own,
                },
            }
        } else if ways.len() == 1 {
            placed.get(&(keys::KEY_BASE + keys::move_key(ways.chars().next().unwrap_or('u')))).copied().unwrap_or(own)
        } else if let Some(l) = label {
            let text: String = ways.chars().filter_map(|w| char::from_u32(keys::move_key(w))).collect();
            match made.get(&text) {
                Some(&g) => g,
                None => match word_key(&text, b, l) {
                    Some((o, over)) => {
                        let g = game.add(&format!("stick_{}", text.to_lowercase()), &o, width0 + round_even(over));
                        made.insert(text, g);
                        g
                    }
                    None => own,
                },
            }
        } else {
            own
        };
        placed.insert(keys::STICK_KEYS + i as u32, g);
    }
    Ok((placed, widths))
}

#[cfg(test)]
impl Contour {
    fn bounds_check(&self) -> (f64, f64, f64, f64) {
        Outline { contours: vec![self.clone()] }.bounds().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mouse_key_is_a_key_a_mouse_cut_out_and_a_button() {
        let o = mouse("left", (0.0, 0.0, 100.0, 100.0));
        assert_eq!(o.contours.len(), 3);
        let areas: Vec<f64> = o.contours.iter().map(Outline::contour_area).collect();
        assert!(areas[0] > 0.0 && areas[1] < 0.0 && areas[2] > 0.0, "{areas:?}");
        let r = rrect(0.0, 0.0, 10.0, 4.0, 3.0);
        assert_eq!(r.bounds_check(), (0.0, 0.0, 10.0, 4.0));
    }
}

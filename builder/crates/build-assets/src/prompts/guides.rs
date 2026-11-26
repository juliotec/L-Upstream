//! The pause menu's controls page's leader lines (`ControllerGuide_Pro_00` and `_01`, BC4, white on black, drawn
//! over the controller's picture): from the labels' bars to the picture's parts, routed round the other parts with
//! the fewest bends, one line per pad input for each Switch button's label, chosen by the controls when the game
//! serves the texture.

use super::faces::{Parts, sw};
use crate::raster::{Filter, Image};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::rc::Rc;

/// The page's two textures of lines that follow the controls.
pub const GUIDES: [&str; 2] = ["ControllerGuide_Pro_00", "ControllerGuide_Pro_01"];
/// The page's line textures a face may give by hand (02 to 04: ZR, the d-pad, L and R, over the game's bars).
pub const HAND_GUIDES: [&str; 5] = [
    "ControllerGuide_Pro_00",
    "ControllerGuide_Pro_01",
    "ControllerGuide_Pro_02",
    "ControllerGuide_Pro_03",
    "ControllerGuide_Pro_04",
];
/// The picture's pixels to the lines' pixels: scale, then offset.
const GUIDE_SCALE: f64 = 1.1136;
const GUIDE_AT: (f64, f64) = (131.3, 462.6);
/// The labels' bars: the left ones' right edge, the right ones' left edge.
pub const BARS: (i64, i64) = (147, 1407);

/// What a line ends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// A Switch button's label: the part of the pad input bound to it.
    Switch(&'static str),
    /// A pad input's (fixed: the sticks).
    Pad(&'static str),
}

/// Each line: its texture, bar (0 left, 1 right), row (its top; 6 pixels thick) and end.
pub const GUIDE_LINES: [(usize, usize, i64, End); 6] = [
    (0, 0, 333, End::Switch("zl")),
    (1, 0, 561, End::Switch("minus")),
    (0, 0, 667, End::Pad("lstick")),
    (0, 1, 558, End::Switch("plus")),
    (0, 1, 713, End::Switch("a")),
    (0, 1, 1225, End::Pad("rstick")),
];
/// The groups a line frames, ending on the frame, when its pad input is one of them; Switch A's label always frames
/// one: the d-pad when A is on it, else the face buttons.
pub const GUIDE_GROUPS: [[&str; 4]; 2] = [["north", "west", "east", "south"], ["up", "left", "right", "down"]];
const GUIDE_FACE: &str = "a";
/// The router: a grid of this many pixels; a bend costs `BEND` pixels of length, crossing a line that stays `CROSS`,
/// coming to a part other than from above `SIDE`; a line keeps `CLEAR` pixels off the other parts (sticks half).
const GRID: i64 = 8;
const BEND: i64 = 64;
const CROSS: i64 = 40;
const SIDE: i64 = 48;
const CLEAR: f64 = 16.0;

fn bar_x(bar: usize) -> i64 {
    if bar == 0 { BARS.0 } else { BARS.1 }
}

/// A part (picture pixels: x, y, r) in the lines' pixels.
fn guide_at(p: (f64, f64, f64)) -> (f64, f64, f64) {
    (p.0 * GUIDE_SCALE + GUIDE_AT.0, p.1 * GUIDE_SCALE + GUIDE_AT.1, p.2 * GUIDE_SCALE)
}

/// A group's frame: its centre and its corners' distance from it (the lines' pixels).
fn frame_at(parts: &Parts, group: &[&str; 4]) -> (f64, f64, f64) {
    let pts: Vec<(f64, f64, f64)> = group.iter().filter_map(|p| parts.get(p)).map(guide_at).collect();
    let cx = pts.iter().map(|p| p.0).sum::<f64>() / 4.0;
    let cy = pts.iter().map(|p| p.1).sum::<f64>() / 4.0;
    let e = pts.iter().map(|&(x, y, r)| (x - cx).abs().max((y - cy).abs()) + r).fold(f64::MIN, f64::max);
    (cx, cy, e + 8.0) // (from the centre to a corner, as the game's)
}

fn py_round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// The lines that stay, widened: the same image for every line drawn over it, so widened once.
#[derive(Default)]
pub struct Widened {
    last: Option<(Rc<Image>, Image)>,
}

impl Widened {
    fn of(&mut self, stays: &Rc<Image>) -> &Image {
        if !self.last.as_ref().is_some_and(|(k, _)| Rc::ptr_eq(k, stays)) {
            self.last = Some((stays.clone(), stays.rank(9, true)));
        }
        &self.last.as_ref().expect("just made").1
    }
}

/// A point of a way, the lines' pixels.
type Pt = (f64, f64);

/// The way of a line from a bar's row to the pad input's part, round the picture's other parts and with the fewest
/// bends, crossing the lines that stay as little as it can: to a stick's centre, to a part's edge (from above best),
/// or to a frame's corner (`frame`: centre, corners' distance).
#[allow(clippy::too_many_arguments)]
fn route(
    size: (usize, usize),
    bar: usize,
    row: i64,
    parts: &Parts,
    pad: &str,
    stays: &Rc<Image>,
    frame: Option<(f64, f64, f64)>,
    widened: &mut Widened,
) -> Option<Vec<Pt>> {
    let g = GRID;
    let (x0, y0) = (bar_x(bar), row + 3);
    let (ilo, ihi) = if bar == 0 { (0, (BARS.1 - x0).div_euclid(g)) } else { (-(x0 - BARS.0).div_euclid(g), 0) };
    let (jlo, jhi) = (-(y0 - 250).div_euclid(g), ((size.1 as i64 - 20).min(1420) - y0).div_euclid(g));
    let (ni, nj) = ((ihi - ilo + 1) as usize, (jhi - jlo + 1).max(0) as usize);
    let xy = |i: i64, j: i64| ((x0 + g * i) as f64, (y0 + g * j) as f64);
    let mut blocked = vec![false; ni * nj];
    let own: Vec<&str> = match frame {
        Some(_) => GUIDE_GROUPS.iter().find(|grp| grp.contains(&pad)).map_or(vec![], |grp| grp.to_vec()),
        None => vec![pad],
    };
    let all = |grp: &[&str; 4]| grp.iter().all(|p| parts.get(p).is_some());
    let frames: Vec<(bool, (f64, f64, f64))> =
        GUIDE_GROUPS.iter().filter(|grp| all(grp)).map(|grp| (grp.contains(&pad), frame_at(parts, grp))).collect();
    for i in ilo..=ihi {
        for j in jlo..=jhi {
            let (x, y) = xy(i, j);
            let mut b = false;
            for &(bx0, by0, bx1, by1) in &parts.boxes {
                let (a, c) = (guide_at((bx0, by0, 0.0)), guide_at((bx1, by1, 0.0)));
                let m = CLEAR / 2.0;
                b |= x > a.0 - m && x < c.0 + m && y > a.1 - m && y < c.1 + m;
            }
            for &(p, part) in &parts.points {
                if own.contains(&p) {
                    continue;
                }
                let (px, py, r) = guide_at(part);
                let clear = if p == "lstick" || p == "rstick" { CLEAR / 2.0 } else { CLEAR };
                b |= (x - px).powi(2) + (y - py).powi(2) < (r + clear).powi(2);
            }
            for &(mine, (cx, cy, e)) in &frames {
                // (a group: inside its frame, which another line may draw, its own inside it)
                b |= if frame.is_some() && mine {
                    (x - cx).powi(2) + (y - cy).powi(2) < (e - 8.0).powi(2)
                } else {
                    (x - cx).abs() + (y - cy).abs() < 1.15 * e + CLEAR
                };
            }
            // (off the bars but on its own row)
            b |= ((x as i64 - BARS.0).abs() < 14 || (x as i64 - BARS.1).abs() < 14) && y as i64 != y0;
            blocked[((i - ilo) as usize) * nj + (j - jlo) as usize] = b;
        }
    }
    let st = widened.of(stays);
    let pen = |i: i64, j: i64| {
        let (x, y) = xy(i, j);
        let (cx, cy) =
            ((x as i64).clamp(0, size.0 as i64 - 1) as usize, (y as i64).clamp(0, size.1 as i64 - 1) as usize);
        if st.at(cx, cy) > 0 { CROSS } else { 0 }
    };
    let inside = |i: i64, j: i64| ilo <= i && i <= ihi && jlo <= j && j <= jhi;
    let is_blocked = |i: i64, j: i64| blocked[((i - ilo) as usize) * nj + (j - jlo) as usize];
    const DIRS: [(i64, i64); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let mut goals: HashMap<(i64, i64, usize), (Pt, i64)> = HashMap::new();
    let mut goal = |ex: f64, ey: f64, d: usize, extra: i64| {
        let i = py_round((ex - x0 as f64) / g as f64) - 2 * DIRS[d].0;
        let j = py_round((ey - y0 as f64) / g as f64) - 2 * DIRS[d].1;
        if (0..2).all(|k| {
            let (ci, cj) = (i + k * DIRS[d].0, j + k * DIRS[d].1);
            inside(ci, cj) && !is_blocked(ci, cj)
        }) {
            goals.insert((i, j, d), ((ex, ey), extra));
        }
    };
    let (x, y, r) = guide_at(parts.get(pad)?);
    if let Some((cx, cy, e)) = frame {
        // (a corner, or beside it on the grid's nearest line, along the frame's side)
        for k in 0..2 {
            let sy = (y0 + g * (((cy - y0 as f64) / g as f64).floor() as i64 + k)) as f64 - cy;
            let sx = (x0 + g * (((cx - x0 as f64) / g as f64).floor() as i64 + k)) as f64 - cx;
            goal(cx + e - sy.abs(), cy + sy, 1, 0);
            goal(cx + sx, cy - e + sx.abs(), 2, SIDE);
            goal(cx - e + sy.abs(), cy + sy, 0, SIDE);
            goal(cx + sx, cy + e - sx.abs(), 3, SIDE);
        }
    } else if pad == "lstick" || pad == "rstick" {
        for d in 0..4 {
            goal(x, y, d, 0);
        }
    } else {
        // (on its edge, as the game's own lines end: 2 pixels into it)
        let e = (r - 2.0).max(0.0);
        for ((ex, ey), d, extra) in
            [((x, y - e), 2, 0), ((x, y + e), 3, SIDE), ((x - e, y), 0, SIDE), ((x + e, y), 1, SIDE)]
        {
            if r != 0.0 || d >= 2 {
                goal(ex, ey, d, extra);
            }
        }
    }
    let start = (0i64, 0i64, if bar == 0 { 0usize } else { 1 });
    let mut best: HashMap<(i64, i64, usize), i64> = HashMap::from([(start, 0)]);
    let mut came: HashMap<(i64, i64, usize), (i64, i64, usize)> = HashMap::new();
    let mut heap = BinaryHeap::from([Reverse((0i64, start))]);
    while let Some(Reverse((c, (i, j, d)))) = heap.pop() {
        if c > *best.get(&(i, j, d)).unwrap_or(&i64::MAX) {
            continue;
        }
        if let Some(&((ex, ey), _)) = goals.get(&(i, j, d)) {
            let mut way = vec![(i, j, d)];
            while let Some(&k) = came.get(way.last().expect("a state")) {
                way.push(k);
            }
            way.reverse();
            let mut pts = vec![xy(way[0].0, way[0].1)];
            for w in way.windows(2) {
                if w[0].2 != w[1].2 {
                    pts.push(xy(w[0].0, w[0].1));
                }
            }
            // (one leg: on its row)
            pts.push(if pts.len() > 1 || (ey - y0 as f64).abs() > g as f64 { (ex, ey) } else { (ex, y0 as f64) });
            if pts.len() > 2 {
                // (the last leg's corner moved onto the end's line: its leg before only longer or shorter)
                let n = pts.len();
                let (cx_, cy_) = pts[n - 2];
                pts[n - 2] = if DIRS[d].0 != 0 { (cx_, ey) } else { (ex, cy_) };
            }
            return Some(square(&pts));
        }
        for (nd, &(dx, dy)) in DIRS.iter().enumerate() {
            if nd == d ^ 1 || (nd != d && j == 0 && i.abs() < 5) {
                continue; // (no turning back; the label's stub first)
            }
            let (ni2, nj2) = (i + dx, j + dy);
            if !inside(ni2, nj2) || is_blocked(ni2, nj2) {
                continue;
            }
            let extra = goals.get(&(ni2, nj2, nd)).map_or(0, |g| g.1);
            let nc = c + g + pen(ni2, nj2) + if nd != d { BEND } else { 0 } + extra;
            if nc < *best.get(&(ni2, nj2, nd)).unwrap_or(&i64::MAX) {
                best.insert((ni2, nj2, nd), nc);
                came.insert((ni2, nj2, nd), (i, j, d));
                heap.push(Reverse((nc, (ni2, nj2, nd))));
            }
        }
    }
    None
}

/// A way's points with every leg level or upright (each point shares x or y with the one before).
fn square(pts: &[Pt]) -> Vec<Pt> {
    let mut out = vec![pts[0]];
    for &(x, y) in &pts[1..] {
        let (px, py) = *out.last().expect("a point");
        if (x, y) == (px, py) {
            continue;
        }
        if x != px && y != py {
            let corner = if out.len() > 1 && out[out.len() - 2].0 == px { (px, y) } else { (x, py) };
            out.push(corner);
        }
        out.push((x, y));
    }
    out
}

/// A line along its way (6 pixels thick, square corners), ending on a dot.
fn draw(size: (usize, usize), way: &[Pt], dot: bool) -> Image {
    let mut im = Image::grey(size.0, size.1);
    for w in way.windows(2) {
        let ((ax, ay), (bx, by)) = (w[0], w[1]);
        let (x0, x1) = (py_round(ax.min(bx)), py_round(ax.max(bx)));
        let (y0, y1) = (py_round(ay.min(by)), py_round(ay.max(by)));
        im.fill_rect(x0 - 3, y0 - 3, x1 + 2, y1 + 2, 255);
    }
    if dot && let Some(&(x, y)) = way.last() {
        im.fill_regular_polygon((py_round(x) as f64, py_round(y) as f64, 11.0), 5, 18.0, 255);
    }
    im
}

/// A leader line from a bar's row to the pad input's part, ending on a dot.
fn line(
    size: (usize, usize),
    bar: usize,
    row: i64,
    parts: &Parts,
    pad: &str,
    stays: &Rc<Image>,
    w: &mut Widened,
) -> Image {
    let way = route(size, bar, row, parts, pad, stays, None, w).unwrap_or_else(|| {
        // (no way round: straight along the row, then up or down to the part's edge)
        let (x, mut y, r) = parts.get(pad).map(guide_at).unwrap_or_default();
        let e = if pad == "lstick" || pad == "rstick" { 0.0 } else { (r - 2.0).max(0.0) };
        y += if (row as f64) < y { -e } else { e };
        vec![(bar_x(bar) as f64, (row + 3) as f64), (x, (row + 3) as f64), (x, y)]
    });
    draw(size, &way, true)
}

/// A group's frame (a square on its corner, rounded, round its parts) and its line from a bar's row.
fn frame(
    size: (usize, usize),
    bar: usize,
    row: i64,
    parts: &Parts,
    group: &[&str; 4],
    stays: &Rc<Image>,
    w: &mut Widened,
) -> Image {
    let (cx, cy, e) = frame_at(parts, group);
    let ss = 4.0;
    let round_ = 0.36 * e;
    let a = (e + (2f64.sqrt() - 1.0) * round_) / 2f64.sqrt();
    let n = (2.0 * a * ss) as usize + 8;
    let mut sq = Image::grey(n, n);
    sq.outline_rounded_rect((4.0, 4.0, n as f64 - 4.0, n as f64 - 4.0), round_ * ss, 6.0 * ss, 255);
    let sq = sq.rotate_bicubic(45.0);
    let sq = sq.resize(sq.width / 4, sq.height / 4, Filter::Lanczos);
    let mut im = Image::grey(size.0, size.1);
    im.place(&sq, py_round(cx - sq.width as f64 / 2.0), py_round(cy - sq.height as f64 / 2.0));
    let way = route(size, bar, row, parts, group[0], stays, Some((cx, cy, e - 3.0)), w).unwrap_or_else(|| {
        let x = if bar == 1 { cx + e - 3.0 } else { cx - e + 3.0 };
        vec![(bar_x(bar) as f64, (row + 3) as f64), (x, (row + 3) as f64)]
    });
    im.lighter(&draw(size, &way, false))
}

/// Where the game's line from `bar` at `row` is drawn in `im` (a guide texture): the shape it makes between the bars,
/// a little wider for its soft edge. `None` when none is found near the label.
fn game_line(im: &Image, bar: usize, row: i64) -> Option<Image> {
    let (x0, x1) = (BARS.0 + 3, BARS.1 - 3); // (off the bars: they join the lines of one side)
    let lit = im.map(|v| if v > 24 { 255 } else { 0 });
    let mut inner = Image::grey(im.width, im.height);
    inner.place(&lit.crop(x0, 0, x1, im.height as i64), x0, 0);
    let x = if bar == 0 { x0 + 2 } else { x1 - 3 } as usize;
    let y = (row - 12..row + 13)
        .filter(|&y| y >= 0 && (y as usize) < im.height && inner.at(x, y as usize) > 0)
        .min_by_key(|&y| (y - row).abs())?;
    inner.flood(x, y as usize, 128);
    let found = inner.map(|v| if v == 128 { 255 } else { 0 }).rank(7, true);
    let mut out = Image::grey(im.width, im.height);
    out.place(&found.crop(BARS.0 + 1, 0, BARS.1, im.height as i64), BARS.0 + 1, 0);
    Some(out)
}

/// For each of the two textures, each Switch button's line for each pad input (those with no part have none); a
/// line shared by several pad inputs (a frame) is one picture.
pub type Moving = Vec<Vec<(&'static str, Vec<(&'static str, Rc<Image>)>)>>;

/// The lines of the page for a face's picture, from the game's (`game`: the two textures decoded): each texture's
/// lines that stay (the Switch face: the game's), and the lines that move ([`Moving`]). `log` hears of a game line
/// not found.
pub fn guide_layers(
    game: &[Image; 2],
    face: &str,
    parts: &Parts,
    log: &mut dyn FnMut(String),
) -> (Vec<Rc<Image>>, Moving) {
    let size = game[0].size();
    let mut w = Widened::default();
    let mut keep: Vec<Rc<Image>> = game
        .iter()
        .map(|im| {
            if face == "switch" {
                return Rc::new(im.clone());
            }
            // (the game's bars)
            let mut k = Image::grey(size.0, size.1);
            k.place(&im.crop(0, 0, BARS.0, size.1 as i64), 0, 0);
            k.place(&im.crop(BARS.1, 0, size.0 as i64, size.1 as i64), BARS.1, 0);
            Rc::new(k)
        })
        .collect();
    // (the lines that stay first: those that move keep off them)
    for &(tx, bar, row, end) in &GUIDE_LINES {
        if let End::Pad(pad) = end
            && face != "switch"
        {
            let l = line(size, bar, row, parts, pad, &keep[tx], &mut w);
            keep[tx] = Rc::new(keep[tx].lighter(&l));
        }
    }
    let mut moving: Moving = vec![Vec::new(), Vec::new()];
    for &(tx, bar, row, end) in &GUIDE_LINES {
        let End::Switch(end) = end else { continue };
        let stays = keep[tx].clone();
        let mut frames: Vec<(&str, Rc<Image>)> = Vec::new();
        for group in &GUIDE_GROUPS {
            if group.iter().all(|p| parts.get(p).is_some()) {
                let im = Rc::new(frame(size, bar, row, parts, group, &stays, &mut w));
                frames.extend(group.iter().map(|&p| (p, im.clone())));
            }
        }
        let framed = |p: &str| frames.iter().find(|f| f.0 == p).map(|f| f.1.clone());
        let mut lines: Vec<(&'static str, Rc<Image>)> = Vec::new();
        for pad in super::faces::PADS {
            if parts.get(pad).is_none() {
                continue;
            }
            let im = match framed(pad) {
                Some(f) => f,
                None if end == GUIDE_FACE && framed("south").is_some() => framed("south").expect("just found"),
                None => Rc::new(line(size, bar, row, parts, pad, &stays, &mut w)),
            };
            lines.push((pad, im));
        }
        if face == "switch" {
            // (the game's own line where the pad input's is longshot's default's: taken out of what stays)
            let default = super::faces::DEFAULT[sw(end)];
            let own = lines.iter().find(|l| l.0 == default).map(|l| l.1.clone());
            let around = game_line(&game[tx], bar, row).unwrap_or_else(|| {
                log(format!("prompts: no game line from bar {bar} at row {row}: nothing taken out there"));
                Image::grey(size.0, size.1)
            });
            let game_own = Rc::new(game[tx].multiply(&around));
            if let Some(own) = own {
                for l in &mut lines {
                    if Rc::ptr_eq(&l.1, &own) {
                        l.1 = game_own.clone();
                    }
                }
            }
            keep[tx] = Rc::new(keep[tx].subtract(&around));
        }
        moving[tx].push((end, lines));
    }
    (keep, moving)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_way_is_level_and_upright() {
        let s = square(&[(0.0, 0.0), (10.0, 0.0), (20.0, 5.0)]);
        assert_eq!(s, [(0.0, 0.0), (10.0, 0.0), (20.0, 0.0), (20.0, 5.0)]);
    }

    #[test]
    fn the_switch_face_s_lines_route_round_its_parts() {
        let parts = super::super::faces::switch_parts();
        let stays = Rc::new(Image::grey(1500, 1500));
        let mut w = Widened::default();
        let way = route((1500, 1500), 0, 667, &parts, "lstick", &stays, None, &mut w).expect("a way");
        assert_eq!(way[0], (BARS.0 as f64, 670.0));
        assert!(way.windows(2).all(|p| p[0].0 == p[1].0 || p[0].1 == p[1].1));
        let again = route((1500, 1500), 0, 667, &parts, "lstick", &stays, None, &mut w).unwrap();
        assert_eq!(way, again);
    }
}

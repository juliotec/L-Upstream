//! Glyph outlines: contours of lines and cubic curves, read from a font, moved, measured and written as a Type 2
//! charstring.

use ab_glyph_rasterizer::{Rasterizer, point};

/// A point, in font units (y up).
pub type P = (f64, f64);

/// A contour's piece after its start, or the point before.
#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    /// A line to the point.
    Line(P),
    /// A cubic curve by two control points to the point.
    Cubic(P, P, P),
}

impl Seg {
    fn end(&self) -> P {
        match *self {
            Seg::Line(p) | Seg::Cubic(_, _, p) => p,
        }
    }
}

/// A closed contour.
#[derive(Clone, Debug, PartialEq)]
pub struct Contour {
    /// Where it starts.
    pub start: P,
    /// Its pieces, in order; it closes back to the start.
    pub segs: Vec<Seg>,
}

/// A glyph's outline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    /// Its contours.
    pub contours: Vec<Contour>,
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.contours.push(Contour { start: (f64::from(x), f64::from(y)), segs: Vec::new() });
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.push(Seg::Line((f64::from(x), f64::from(y))));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // (a quadratic as the cubic it is)
        let p0 = self.last();
        let (q, p) = ((f64::from(x1), f64::from(y1)), (f64::from(x), f64::from(y)));
        let c1 = (p0.0 + 2.0 / 3.0 * (q.0 - p0.0), p0.1 + 2.0 / 3.0 * (q.1 - p0.1));
        let c2 = (p.0 + 2.0 / 3.0 * (q.0 - p.0), p.1 + 2.0 / 3.0 * (q.1 - p.1));
        self.push(Seg::Cubic(c1, c2, p));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let f = f64::from;
        self.push(Seg::Cubic((f(x1), f(y1)), (f(x2), f(y2)), (f(x), f(y))));
    }
    fn close(&mut self) {}
}

fn cubic_extremes(a: f64, b: f64, c: f64, d: f64) -> Vec<f64> {
    // (the roots in (0, 1) of the derivative of the cubic with control values a, b, c, d)
    let (qa, qb, qc) = (3.0 * (-a + 3.0 * b - 3.0 * c + d), 6.0 * (a - 2.0 * b + c), 3.0 * (b - a));
    let mut ts = Vec::new();
    if qa.abs() < 1e-12 {
        if qb.abs() > 1e-12 {
            ts.push(-qc / qb);
        }
    } else {
        let disc = qb * qb - 4.0 * qa * qc;
        if disc >= 0.0 {
            let r = disc.sqrt();
            ts.push((-qb + r) / (2.0 * qa));
            ts.push((-qb - r) / (2.0 * qa));
        }
    }
    ts.into_iter().filter(|t| *t > 0.0 && *t < 1.0).collect()
}

fn cubic_at(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
}

impl Outline {
    /// A font's glyph's outline, in font units.
    pub fn of(face: &ttf_parser::Face<'_>, glyph: ttf_parser::GlyphId) -> Outline {
        let mut o = Outline::default();
        let _ = face.outline_glyph(glyph, &mut o);
        o
    }

    fn push(&mut self, s: Seg) {
        if let Some(c) = self.contours.last_mut() {
            c.segs.push(s);
        }
    }

    fn last(&self) -> P {
        self.contours.last().map_or((0.0, 0.0), |c| c.segs.last().map_or(c.start, Seg::end))
    }

    /// Whether it has no contours.
    pub fn is_empty(&self) -> bool {
        self.contours.is_empty()
    }

    /// Its exact bounds (x0, y0, x1, y1): curves by their extremes, not their control points.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut b: Option<(f64, f64, f64, f64)> = None;
        let mut add = |(x, y): P| {
            b = Some(match b {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        };
        for c in &self.contours {
            add(c.start);
            let mut prev = c.start;
            for s in &c.segs {
                if let Seg::Cubic(c1, c2, p) = *s {
                    for t in cubic_extremes(prev.0, c1.0, c2.0, p.0) {
                        add((cubic_at(prev.0, c1.0, c2.0, p.0, t), cubic_at(prev.1, c1.1, c2.1, p.1, t)));
                    }
                    for t in cubic_extremes(prev.1, c1.1, c2.1, p.1) {
                        add((cubic_at(prev.0, c1.0, c2.0, p.0, t), cubic_at(prev.1, c1.1, c2.1, p.1, t)));
                    }
                }
                add(s.end());
                prev = s.end();
            }
        }
        b
    }

    /// Each point `(s x + dx, s y + dy)`.
    pub fn transformed(&self, s: f64, dx: f64, dy: f64) -> Outline {
        let t = |(x, y): P| (s * x + dx, s * y + dy);
        Outline {
            contours: self
                .contours
                .iter()
                .map(|c| Contour {
                    start: t(c.start),
                    segs: c
                        .segs
                        .iter()
                        .map(|s| match *s {
                            Seg::Line(p) => Seg::Line(t(p)),
                            Seg::Cubic(a, b, p) => Seg::Cubic(t(a), t(b), t(p)),
                        })
                        .collect(),
                })
                .collect(),
        }
    }

    /// Its contours after `other`'s.
    pub fn extend(&mut self, other: Outline) {
        self.contours.extend(other.contours);
    }

    /// As a Type 2 charstring: its width operand first (`width - nominal`, left out when `width` is `default`), each
    /// point rounded to a whole unit, then `endchar`.
    pub fn charstring(&self, width: i32, default: i32, nominal: i32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut width = (width != default).then_some(width.saturating_sub(nominal));
        let mut at = (0i32, 0i32);
        // (the differences saturate: a crafted outline of coordinates near i32's ends is cut there, not wrapped)
        let r = |(x, y): P| ((x + 0.5).floor() as i32, (y + 0.5).floor() as i32);
        let mut args: Vec<i32> = Vec::new();
        let mut op: Option<u8> = None;
        let flush = |out: &mut Vec<u8>, args: &mut Vec<i32>, op: &mut Option<u8>| {
            if let Some(o) = op.take() {
                for &a in args.iter() {
                    number(out, a);
                }
                out.push(o);
            }
            args.clear();
        };
        for c in &self.contours {
            flush(&mut out, &mut args, &mut op);
            let p = r(c.start);
            if let Some(w) = width.take() {
                number(&mut out, w);
            }
            number(&mut out, p.0.saturating_sub(at.0));
            number(&mut out, p.1.saturating_sub(at.1));
            out.push(21); // rmoveto
            at = p;
            for s in &c.segs {
                let (code, pts): (u8, Vec<P>) = match *s {
                    // (a line of no length once rounded is left out)
                    Seg::Line(p) if r(p) == at => continue,
                    Seg::Line(p) => (5, vec![p]),
                    Seg::Cubic(a, b, p) => (8, vec![a, b, p]),
                };
                // (one operator for a run of the same, under the stack's 48 arguments)
                if op != Some(code) || args.len() + 2 * pts.len() > 48 {
                    flush(&mut out, &mut args, &mut op);
                    op = Some(code);
                }
                for p in pts {
                    let p = r(p);
                    let d = (p.0.saturating_sub(at.0), p.1.saturating_sub(at.1));
                    at = p;
                    // (a level or upright line straight on from one the same way: one line, as fontTools writes it)
                    let n = args.len();
                    if code == 5 && n >= 2 {
                        let (px, py) = (args[n - 2], args[n - 1]);
                        if (px == 0 && d.0 == 0 && py.signum() == d.1.signum())
                            || (py == 0 && d.1 == 0 && px.signum() == d.0.signum())
                        {
                            args[n - 2] = args[n - 2].saturating_add(d.0);
                            args[n - 1] = args[n - 1].saturating_add(d.1);
                            continue;
                        }
                    }
                    args.extend([d.0, d.1]);
                }
            }
        }
        flush(&mut out, &mut args, &mut op);
        if let Some(w) = width {
            number(&mut out, w);
        }
        out.push(14); // endchar
        out
    }

    /// Its signed area (positive: counter-clockwise with y up), the curves flattened finely.
    pub fn contour_area(c: &Contour) -> f64 {
        let mut pts = vec![c.start];
        let mut prev = c.start;
        for s in &c.segs {
            match *s {
                Seg::Line(p) => pts.push(p),
                Seg::Cubic(a, b, p) => {
                    for k in 1..=16 {
                        let t = f64::from(k) / 16.0;
                        pts.push((cubic_at(prev.0, a.0, b.0, p.0, t), cubic_at(prev.1, a.1, b.1, p.1, t)));
                    }
                }
            }
            prev = s.end();
        }
        let n = pts.len();
        (0..n).map(|i| pts[i].0 * pts[(i + 1) % n].1 - pts[(i + 1) % n].0 * pts[i].1).sum::<f64>() / 2.0
    }

    /// The outline's coverage as a grey image: `scale` pixels a unit, y down, the point (x0, y1) of the font at the
    /// image's top left, `width` x `height` pixels. Exact area coverage, the contours by the nonzero rule's sum.
    pub fn rasterize(&self, scale: f64, x0: f64, y1: f64, width: usize, height: usize) -> crate::raster::Image {
        let mut r = Rasterizer::new(width, height);
        let tr = |(x, y): P| point(((x - x0) * scale) as f32, ((y1 - y) * scale) as f32);
        for c in &self.contours {
            let mut prev = c.start;
            for s in c.segs.iter().cloned().chain(std::iter::once(Seg::Line(c.start))) {
                match s {
                    Seg::Line(p) => r.draw_line(tr(prev), tr(p)),
                    Seg::Cubic(a, b, p) => r.draw_cubic(tr(prev), tr(a), tr(b), tr(p)),
                }
                prev = s.end();
            }
        }
        let mut im = crate::raster::Image::grey(width, height);
        r.for_each_pixel_2d(|x, y, v| {
            im.data[y as usize * width + x as usize] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        });
        im
    }
}

impl Contour {
    /// The same contour the other way round, from the same start.
    pub fn reversed(&self) -> Contour {
        let mut pts = vec![self.start];
        pts.extend(self.segs.iter().map(Seg::end));
        let mut segs = Vec::with_capacity(self.segs.len());
        for (i, s) in self.segs.iter().enumerate().rev() {
            let to = pts[i];
            segs.push(match *s {
                Seg::Line(_) => Seg::Line(to),
                Seg::Cubic(a, b, _) => Seg::Cubic(b, a, to),
            });
        }
        // (the closing line, from the last point back to the start, becomes the first piece, unless the contour
        // ended on its start; the last piece back to the start, when a line, is the reversed contour's own close)
        let last = *pts.last().unwrap_or(&self.start);
        let mut out = if last == self.start { Vec::new() } else { vec![Seg::Line(last)] };
        out.extend(segs);
        if matches!(out.last(), Some(Seg::Line(p)) if *p == self.start) {
            out.pop();
        }
        Contour { start: self.start, segs: out }
    }
}

/// A charstring's integer operand.
fn number(out: &mut Vec<u8>, v: i32) {
    match v {
        -107..=107 => out.push((v + 139) as u8),
        108..=1131 => {
            let w = v - 108;
            out.extend([(w / 256 + 247) as u8, (w % 256) as u8]);
        }
        -1131..=-108 => {
            let w = -v - 108;
            out.extend([(w / 256 + 251) as u8, (w % 256) as u8]);
        }
        _ => {
            let w = v.clamp(-32768, 32767) as i16;
            out.push(28);
            out.extend(w.to_be_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, x1: f64, y1: f64) -> Contour {
        Contour { start: (x0, y0), segs: vec![Seg::Line((x1, y0)), Seg::Line((x1, y1)), Seg::Line((x0, y1))] }
    }

    #[test]
    fn bounds_area_and_reversal() {
        let mut o = Outline { contours: vec![square(0.0, 0.0, 10.0, 10.0)] };
        assert_eq!(o.bounds(), Some((0.0, 0.0, 10.0, 10.0)));
        assert!(Outline::contour_area(&o.contours[0]) > 99.9);
        let r = o.contours[0].reversed();
        assert!(Outline::contour_area(&r) < -99.9);
        assert_eq!(r.reversed(), o.contours[0]);
        // (a curve's bounds are its extremes, inside its control points)
        o.contours
            .push(Contour { start: (0.0, 20.0), segs: vec![Seg::Cubic((0.0, 40.0), (10.0, 40.0), (10.0, 20.0))] });
        assert_eq!(o.bounds().unwrap().3, 35.0);
    }

    #[test]
    fn a_charstring_of_whole_units() {
        let o = Outline { contours: vec![square(0.4, 0.0, 10.0, 200.0)] };
        let cs = o.charstring(500, 500, 0);
        // rmoveto 0 0, rlineto 10 0 0 200 -10 0, endchar
        assert_eq!(cs, [139, 139, 21, 149, 139, 139, 247, 92, 129, 139, 5, 14]);
        assert_eq!(Outline::default().charstring(600, 500, 100), [248, 136, 14]);
    }

    #[test]
    fn coverage_of_a_square() {
        let o = Outline { contours: vec![square(1.0, 1.0, 3.0, 3.0)] };
        let im = o.rasterize(1.0, 0.0, 4.0, 4, 4);
        assert_eq!(im.data, [0, 0, 0, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 0, 0, 0]);
    }
}

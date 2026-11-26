//! Shapes drawn into grey images, as Pillow's `ImageDraw` draws them (no smoothing: a pixel is in or out), and
//! Pillow's bicubic rotation.

use crate::raster::Image;

impl Image {
    /// The rectangle of pixels x0..=x1, y0..=y1 (both ends in, as Pillow's `rectangle`) set to `v`, clipped.
    pub fn fill_rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, v: u8) {
        let (w, h) = (self.width as i64, self.height as i64);
        for y in y0.max(0)..=y1.min(h - 1) {
            for x in x0.max(0)..=x1.min(w - 1) {
                self.data[(y * w + x) as usize] = v;
            }
        }
    }

    /// The polygon of `points` filled with `v`, as Pillow fills one: the corners cut to whole pixels, each row's
    /// crossings of the edges at its integer y, the pixels between each pair (their ends rounded to the nearest, a half
    /// inwards).
    pub fn fill_polygon(&mut self, points: &[(f64, f64)], v: u8) {
        let pts: Vec<(i64, i64)> = points.iter().map(|&(x, y)| (x.floor() as i64, y.floor() as i64)).collect();
        let n = pts.len();
        if n < 2 {
            return;
        }
        // (the edges, each from its lower y to its higher, those of no height left out)
        let mut edges = Vec::new();
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            if a == b {
                continue;
            }
            let (lo, hi) = if a.1 <= b.1 { (a, b) } else { (b, a) };
            if lo.1 == hi.1 {
                continue;
            }
            let dx = (hi.0 - lo.0) as f32 / (hi.1 - lo.1) as f32;
            edges.push((lo.1, hi.1, lo.0 as f32, lo.1, dx));
        }
        let ymin = edges.iter().map(|e| e.0).min().unwrap_or(0).max(0);
        let ymax = edges.iter().map(|e| e.1).max().unwrap_or(0).min(self.height as i64);
        let mut y = ymin;
        while y <= ymax {
            let mut xx: Vec<f32> = Vec::new();
            for &(e_ymin, e_ymax, x0, y0, dx) in &edges {
                if y >= e_ymin && y <= e_ymax {
                    let x = (y - y0) as f32 * dx + x0;
                    xx.push(x);
                    if y == e_ymax && y < ymax {
                        xx.push(x);
                    }
                }
            }
            xx.sort_by(f32::total_cmp);
            for pair in xx.as_chunks::<2>().0 {
                let (xs, xe) = ((pair[0] + 0.5).floor() as i64, (pair[1] - 0.5).ceil() as i64);
                if xe >= xs && y < self.height as i64 {
                    self.fill_rect(xs, y, xe, y, v);
                }
            }
            y += 1;
        }
    }

    /// A regular polygon of `n` sides round (x, y), its corners `r` from it, turned by `rotation` degrees, filled
    /// (Pillow's `regular_polygon`).
    pub fn fill_regular_polygon(&mut self, (x, y, r): (f64, f64, f64), n: usize, rotation: f64, v: u8) {
        let degrees = 360.0 / n as f64;
        let mut angle = (270.0 - 0.5 * degrees) + rotation;
        let mut pts = Vec::with_capacity(n);
        let round2 = |v: f64| (v * 100.0).round_ties_even() / 100.0;
        for _ in 0..n {
            let a = (360.0 - angle).to_radians();
            pts.push((round2(r * a.cos() + x), round2(r * a.sin() + y)));
            angle += degrees;
            if angle > 360.0 {
                angle -= 360.0;
            }
        }
        self.fill_polygon(&pts, v);
    }

    /// The ellipse in the box x0..=x1, y0..=y1 filled with `colour`: the pixels whose centres are inside it.
    pub fn fill_ellipse(&mut self, (x0, y0, x1, y1): (i64, i64, i64, i64), colour: &[u8]) {
        let (cx, cy) = ((x0 + x1 + 1) as f64 / 2.0, (y0 + y1 + 1) as f64 / 2.0);
        let (rx, ry) = ((x1 - x0 + 1) as f64 / 2.0, (y1 - y0 + 1) as f64 / 2.0);
        for y in y0.max(0)..=y1.min(self.height as i64 - 1) {
            for x in x0.max(0)..=x1.min(self.width as i64 - 1) {
                let (dx, dy) = ((x as f64 + 0.5 - cx) / rx, (y as f64 + 0.5 - cy) / ry);
                if dx * dx + dy * dy <= 1.0 {
                    self.px_mut(x as usize, y as usize).copy_from_slice(colour);
                }
            }
        }
    }

    /// The outline (`width` pixels thick, inwards) of the rounded rectangle x0..=x1, y0..=y1 with corners of `radius`
    /// set to `v`: the pixels whose centres are inside it and not inside the rectangle `width` in.
    pub fn outline_rounded_rect(&mut self, (x0, y0, x1, y1): (f64, f64, f64, f64), radius: f64, width: f64, v: u8) {
        let inside = |px: f64, py: f64, x0: f64, y0: f64, x1: f64, y1: f64, r: f64| {
            if px < x0 || px > x1 || py < y0 || py > y1 {
                return false;
            }
            let r = r.max(0.0).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
            let cx = px.clamp(x0 + r, x1 - r);
            let cy = py.clamp(y0 + r, y1 - r);
            (px - cx).powi(2) + (py - cy).powi(2) <= r * r
        };
        let (gx0, gy0, gx1, gy1) = (x0, y0, x1 + 1.0, y1 + 1.0); // (inclusive pixel ends: the box to their far edge)
        for y in (y0.floor().max(0.0) as usize)..((y1 + 1.0).ceil().min(self.height as f64) as usize) {
            for x in (x0.floor().max(0.0) as usize)..((x1 + 1.0).ceil().min(self.width as f64) as usize) {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                if inside(px, py, gx0, gy0, gx1, gy1, radius)
                    && !inside(px, py, gx0 + width, gy0 + width, gx1 - width, gy1 - width, radius - width)
                {
                    self.data[y * self.width + x] = v;
                }
            }
        }
    }

    /// Turned `angle` degrees counter-clockwise round its centre, the canvas grown to hold it all, by Pillow's
    /// bicubic filter (`rotate(angle, BICUBIC, expand=True)`); what comes from outside is 0.
    pub fn rotate_bicubic(&self, angle: f64) -> Image {
        let (w, h) = (self.width as f64, self.height as f64);
        let r = |v: f64| (v * 1e15).round() / 1e15;
        let a = -(angle % 360.0).to_radians();
        let mut m = [r(a.cos()), r(a.sin()), 0.0, r(-a.sin()), r(a.cos()), 0.0];
        let tr = |x: f64, y: f64, m: &[f64; 6]| (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let (tx, ty) = tr(-cx, -cy, &m);
        m[2] = tx + cx;
        m[5] = ty + cy;
        let corners: Vec<(f64, f64)> =
            [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].iter().map(|&(x, y)| tr(x, y, &m)).collect();
        let span = |f: fn(&(f64, f64)) -> f64| {
            let v: Vec<f64> = corners.iter().map(f).collect();
            v.iter().cloned().fold(f64::MIN, f64::max).ceil() - v.iter().cloned().fold(f64::MAX, f64::min).floor()
        };
        let (nw, nh) = (span(|p| p.0), span(|p| p.1));
        let (tx, ty) = tr(-(nw - w) / 2.0, -(nh - h) / 2.0, &m);
        m[2] = tx;
        m[5] = ty;
        let (ow, oh) = (nw as usize, nh as usize);
        let mut out = Image::grey(ow, oh);
        let (iw, ih) = (self.width as i64, self.height as i64);
        let at = |x: i64, y: i64| f64::from(self.at(x.clamp(0, iw - 1) as usize, y.clamp(0, ih - 1) as usize));
        let cubic = |v1: f64, v2: f64, v3: f64, v4: f64, d: f64| {
            let (p1, p2, p3, p4) = (v2, -v1 + v3, 2.0 * (v1 - v2) + v3 - v4, -v1 + v2 - v3 + v4);
            p1 + d * (p2 + d * (p3 + d * p4))
        };
        for y in 0..oh {
            for x in 0..ow {
                let (xin, yin) = tr(x as f64 + 0.5, y as f64 + 0.5, &m);
                if xin < 0.0 || xin >= w || yin < 0.0 || yin >= h {
                    continue;
                }
                let (xin, yin) = (xin - 0.5, yin - 0.5);
                let (fx, fy) = (xin.floor() as i64, yin.floor() as i64);
                let (dx, dy) = (xin - fx as f64, yin - fy as f64);
                let (x0, y0) = (fx - 1, fy - 1);
                let row = |yy: i64| cubic(at(x0, yy), at(x0 + 1, yy), at(x0 + 2, yy), at(x0 + 3, yy), dx);
                let v1 = row(y0);
                let v2 = if y0 + 1 >= 0 && y0 + 1 < ih { row(y0 + 1) } else { v1 };
                let v3 = if y0 + 2 >= 0 && y0 + 2 < ih { row(y0 + 2) } else { v2 };
                let v4 = if y0 + 3 >= 0 && y0 + 3 < ih { row(y0 + 3) } else { v3 };
                let v = cubic(v1, v2, v3, v4, dy);
                out.data[y * ow + x] = if v <= 0.0 {
                    0
                } else if v >= 255.0 {
                    255
                } else {
                    v as u8
                };
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_include_both_ends() {
        let mut im = Image::grey(10, 10);
        im.fill_rect(2, 3, 4, 3, 255);
        assert_eq!(im.data.iter().filter(|&&v| v == 255).count(), 3);
    }

    #[test]
    fn a_square_rotated_a_quarter_turn_is_itself() {
        let mut im = Image::grey(8, 8);
        im.fill_rect(2, 2, 5, 5, 200);
        let r = im.rotate_bicubic(90.0);
        assert_eq!(r.size(), (8, 8));
        assert_eq!(r, im);
        let d = im.rotate_bicubic(45.0);
        assert_eq!(d.size(), (12, 12));
    }

    #[test]
    fn a_polygon_and_an_ellipse_fill_their_inside() {
        let mut im = Image::grey(30, 30);
        im.fill_regular_polygon((15.0, 15.0, 11.0), 5, 18.0, 255);
        assert_eq!(im.at(15, 15), 255);
        assert_eq!(im.at(1, 1), 0);
        let mut e = Image::rgba(10, 10);
        e.fill_ellipse((0, 0, 9, 9), &[1, 2, 3, 4]);
        assert_eq!(e.px(5, 5), [1, 2, 3, 4]);
        assert_eq!(e.px(0, 0), [0, 0, 0, 0]);
    }
}

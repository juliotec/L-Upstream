//! The pictures' pixels: 8-bit images of one channel (grey, a mask) or four (RGBA, straight alpha), and what the
//! prompts and the icon do with them.
//!
//! The operations are Pillow's, to the rounding where it matters, so a picture made here looks as the Python builder's
//! did: [`Image::resize`] is Pillow's separable resampling (its filters, supports and 22-bit fixed-point weights, RGBA
//! premultiplied around it), [`Image::paste`] its mask blend, [`Image::gaussian_blur`] its three box blurs, and the
//! channel operations (`lighter`, `multiply`, `subtract`) its `ImageChops`.

use build_core::{Error, Result};

/// The most pixels a picture read from a file may have (4096 x 4096).
pub const PICTURE_MOST: u64 = 4096 * 4096;

/// An image: `width` x `height` pixels of `channels` bytes (1: grey; 4: RGBA, straight alpha), row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Its width in pixels.
    pub width: usize,
    /// Its height in pixels.
    pub height: usize,
    /// Bytes a pixel: 1 or 4.
    pub channels: usize,
    /// The pixels.
    pub data: Vec<u8>,
}

/// A resampling filter (Pillow's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// Each output pixel the mean of the input pixels it covers.
    Box,
    /// The cubic convolution with a = -0.5.
    Bicubic,
    /// Lanczos with 3 lobes.
    Lanczos,
}

impl Filter {
    fn support(self) -> f64 {
        match self {
            Filter::Box => 0.5,
            Filter::Bicubic => 2.0,
            Filter::Lanczos => 3.0,
        }
    }

    fn weight(self, x: f64) -> f64 {
        fn sinc(x: f64) -> f64 {
            if x == 0.0 {
                return 1.0;
            }
            let x = x * std::f64::consts::PI;
            x.sin() / x
        }
        match self {
            Filter::Box => f64::from(u8::from(x > -0.5 && x <= 0.5)),
            Filter::Bicubic => {
                let a = -0.5;
                let x = x.abs();
                if x < 1.0 {
                    ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
                } else if x < 2.0 {
                    (((x - 5.0) * x + 8.0) * x - 4.0) * a
                } else {
                    0.0
                }
            }
            Filter::Lanczos => {
                if (-3.0..3.0).contains(&x) {
                    sinc(x) * sinc(x / 3.0)
                } else {
                    0.0
                }
            }
        }
    }
}

/// Pillow's `MULDIV255`: `a * b / 255`, rounded.
pub fn muldiv255(a: u32, b: u32) -> u8 {
    let t = a * b + 128;
    (((t >> 8) + t) >> 8) as u8
}

const PRECISION_BITS: u32 = 32 - 8 - 2;

/// The weights of each output pixel of a pass: (first input pixel, the weights in 22-bit fixed point).
fn coefficients(in_size: usize, out_size: usize, filter: Filter) -> Vec<(usize, Vec<i32>)> {
    let scale = in_size as f64 / out_size as f64;
    let filterscale = scale.max(1.0);
    let support = filter.support() * filterscale;
    (0..out_size)
        .map(|xx| {
            let center = (xx as f64 + 0.5) * scale;
            let ss = 1.0 / filterscale;
            let xmin = ((center - support + 0.5) as i64).max(0) as usize;
            let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize;
            let w: Vec<f64> = (xmin..xmax.max(xmin)).map(|x| filter.weight((x as f64 - center + 0.5) * ss)).collect();
            let total: f64 = w.iter().sum();
            let fixed = w
                .iter()
                .map(|&k| {
                    let k = if total != 0.0 { k / total } else { k };
                    let s = k * f64::from(1u32 << PRECISION_BITS);
                    if k < 0.0 { (s - 0.5) as i32 } else { (s + 0.5) as i32 }
                })
                .collect();
            (xmin, fixed)
        })
        .collect()
}

fn clip8(v: i64) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

impl Image {
    /// A `width` x `height` image of `channels`, every pixel `fill`.
    pub fn new(width: usize, height: usize, channels: usize, fill: &[u8]) -> Image {
        debug_assert_eq!(fill.len(), channels);
        let data = fill.iter().copied().cycle().take(width * height * channels).collect();
        Image { width, height, channels, data }
    }

    /// A grey image, all 0.
    pub fn grey(width: usize, height: usize) -> Image {
        Image::new(width, height, 1, &[0])
    }

    /// An RGBA image, all transparent black.
    pub fn rgba(width: usize, height: usize) -> Image {
        Image::new(width, height, 4, &[0; 4])
    }

    /// Its size.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// The pixel at (x, y).
    pub fn px(&self, x: usize, y: usize) -> &[u8] {
        let at = (y * self.width + x) * self.channels;
        &self.data[at..at + self.channels]
    }

    /// The pixel at (x, y), to change.
    pub fn px_mut(&mut self, x: usize, y: usize) -> &mut [u8] {
        let at = (y * self.width + x) * self.channels;
        &mut self.data[at..at + self.channels]
    }

    /// The grey value at (x, y) (a grey image's).
    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.width + x]
    }

    /// As RGBA: a grey image's value in each colour, opaque.
    pub fn to_rgba(&self) -> Image {
        if self.channels == 4 {
            return self.clone();
        }
        let data = self.data.iter().flat_map(|&v| [v, v, v, 255]).collect();
        Image { width: self.width, height: self.height, channels: 4, data }
    }

    /// As grey: Pillow's `L` of RGB, (299 R + 587 G + 114 B) / 1000 rounded (alpha dropped).
    pub fn to_grey(&self) -> Image {
        if self.channels == 1 {
            return self.clone();
        }
        let data = self
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                ((u32::from(p[0]) * 19595 + u32::from(p[1]) * 38470 + u32::from(p[2]) * 7471 + 0x8000) >> 16) as u8
            })
            .collect();
        Image { width: self.width, height: self.height, channels: 1, data }
    }

    /// One channel as a grey image.
    pub fn channel(&self, c: usize) -> Image {
        let data = self.data.chunks_exact(self.channels).map(|p| p[c]).collect();
        Image { width: self.width, height: self.height, channels: 1, data }
    }

    /// Each value mapped through `f` (Pillow's `point`).
    pub fn map(&self, f: impl Fn(u8) -> u8) -> Image {
        let table: Vec<u8> = (0..=255u8).map(&f).collect();
        Image { data: self.data.iter().map(|&v| table[usize::from(v)]).collect(), ..self.clone() }
    }

    /// 255 where above `level`, else 0.
    pub fn threshold(&self, level: u8) -> Image {
        self.map(|v| if v > level { 255 } else { 0 })
    }

    /// Each value 255 less itself.
    pub fn invert(&self) -> Image {
        self.map(|v| 255 - v)
    }

    fn zip(&self, other: &Image, f: impl Fn(u8, u8) -> u8) -> Image {
        debug_assert_eq!((self.size(), self.channels), (other.size(), other.channels));
        Image { data: self.data.iter().zip(&other.data).map(|(&a, &b)| f(a, b)).collect(), ..self.clone() }
    }

    /// The lighter of the two at each pixel.
    pub fn lighter(&self, other: &Image) -> Image {
        self.zip(other, u8::max)
    }

    /// The product of the two, over 255 (Pillow's `multiply`).
    pub fn multiply(&self, other: &Image) -> Image {
        self.zip(other, |a, b| ((u32::from(a) * u32::from(b)) / 255) as u8)
    }

    /// This less the other, at least 0.
    pub fn subtract(&self, other: &Image) -> Image {
        self.zip(other, u8::saturating_sub)
    }

    /// The box (x0, y0, x1, y1) of the pixels that are not all 0, or `None`.
    pub fn bbox(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..self.height {
            let row = &self.data[y * self.width * self.channels..(y + 1) * self.width * self.channels];
            for (x, p) in row.chunks_exact(self.channels).enumerate() {
                if p.iter().any(|&v| v != 0) {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        (x0 != usize::MAX).then_some((x0, y0, x1, y1))
    }

    /// The box (x0, y0, x1, y1), which may reach past the image (Pillow's `crop`: what is outside is 0).
    pub fn crop(&self, x0: i64, y0: i64, x1: i64, y1: i64) -> Image {
        let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
        let mut out = Image { width: w, height: h, channels: self.channels, data: vec![0; w * h * self.channels] };
        out.place(self, -x0, -y0);
        out
    }

    /// `src` copied with its top left at (x, y), clipped (Pillow's `paste` without a mask).
    pub fn place(&mut self, src: &Image, x: i64, y: i64) {
        debug_assert_eq!(self.channels, src.channels);
        let c = self.channels;
        let (sx0, sy0) = ((-x).max(0) as usize, (-y).max(0) as usize);
        let (dx0, dy0) = (x.max(0) as usize, y.max(0) as usize);
        if sx0 >= src.width || sy0 >= src.height || dx0 >= self.width || dy0 >= self.height {
            return;
        }
        let w = (src.width - sx0).min(self.width - dx0);
        for row in 0..(src.height - sy0).min(self.height - dy0) {
            let s = ((sy0 + row) * src.width + sx0) * c;
            let d = ((dy0 + row) * self.width + dx0) * c;
            self.data[d..d + w * c].copy_from_slice(&src.data[s..s + w * c]);
        }
    }

    /// `colour` (one value a channel) blended in where `mask` (grey) is, its top left at (x, y) (Pillow's
    /// `paste(colour, box, mask)`).
    pub fn paste_colour(&mut self, colour: &[u8], mask: &Image, x: i64, y: i64) {
        let fill = Image::new(mask.width, mask.height, self.channels, colour);
        self.paste(&fill, mask, x, y);
    }

    /// `src` blended in by `mask` (grey, `src`'s size), its top left at (x, y): each channel, alpha too, `out +
    /// (src - out) * mask / 255`, rounded as Pillow rounds it.
    pub fn paste(&mut self, src: &Image, mask: &Image, x: i64, y: i64) {
        debug_assert_eq!(src.size(), mask.size());
        let c = self.channels;
        for my in 0..mask.height {
            let ty = y + my as i64;
            if ty < 0 || ty >= self.height as i64 {
                continue;
            }
            for mx in 0..mask.width {
                let tx = x + mx as i64;
                if tx < 0 || tx >= self.width as i64 {
                    continue;
                }
                let m = u32::from(mask.at(mx, my));
                if m == 0 {
                    continue;
                }
                let s = src.px(mx, my).to_vec();
                let d = self.px_mut(tx as usize, ty as usize);
                for k in 0..c {
                    let blended = u32::from(d[k]) * (255 - m) + u32::from(s[k]) * m;
                    let t = blended + 128;
                    d[k] = (((t >> 8) + t) >> 8) as u8;
                }
            }
        }
    }

    /// Resized to `width` x `height` by `filter`: Pillow's two passes, horizontal first. RGBA is premultiplied around
    /// it, as Pillow does.
    pub fn resize(&self, width: usize, height: usize, filter: Filter) -> Image {
        if self.size() == (width, height) {
            return self.clone();
        }
        let premultiplied = self.channels == 4;
        let mut im = if premultiplied { self.premultiplied() } else { self.clone() };
        let c = im.channels;
        let vertical = coefficients(im.height, height, filter);
        let first = vertical.first().map_or(0, |v| v.0);
        let last = vertical.last().map_or(0, |v| v.0 + v.1.len());
        if width != im.width {
            let horizontal = coefficients(im.width, width, filter);
            let rows = last - first;
            let mut out = vec![0u8; width * rows * c];
            for row in 0..rows {
                let src = &im.data[(first + row) * im.width * c..(first + row + 1) * im.width * c];
                for (xx, (xmin, k)) in horizontal.iter().enumerate() {
                    for ch in 0..c {
                        let mut ss = 1i64 << (PRECISION_BITS - 1);
                        for (i, &w) in k.iter().enumerate() {
                            ss += i64::from(src[(xmin + i) * c + ch]) * i64::from(w);
                        }
                        out[(row * width + xx) * c + ch] = clip8(ss);
                    }
                }
            }
            im = Image { width, height: rows, channels: c, data: out };
        } else {
            im = im.crop(0, first as i64, im.width as i64, last as i64);
        }
        if height != self.height {
            let mut out = vec![0u8; width * height * c];
            for (yy, (ymin, k)) in vertical.iter().enumerate() {
                for x in 0..width {
                    for ch in 0..c {
                        let mut ss = 1i64 << (PRECISION_BITS - 1);
                        for (i, &w) in k.iter().enumerate() {
                            ss += i64::from(im.data[((ymin - first + i) * width + x) * c + ch]) * i64::from(w);
                        }
                        out[(yy * width + x) * c + ch] = clip8(ss);
                    }
                }
            }
            im = Image { width, height, channels: c, data: out };
        }
        if premultiplied { im.unpremultiplied() } else { im }
    }

    fn premultiplied(&self) -> Image {
        let mut out = self.clone();
        for p in out.data.as_chunks_mut::<4>().0.iter_mut() {
            let a = u32::from(p[3]);
            for v in &mut p[..3] {
                *v = muldiv255(u32::from(*v), a);
            }
        }
        out
    }

    fn unpremultiplied(&self) -> Image {
        let mut out = self.clone();
        for p in out.data.as_chunks_mut::<4>().0.iter_mut() {
            let a = u32::from(p[3]);
            if a != 0 && a != 255 {
                for v in &mut p[..3] {
                    *v = ((255 * u32::from(*v)) / a).min(255) as u8;
                }
            }
        }
        out
    }

    /// Each pixel the most (`max`) or least of the `size` x `size` square round it, the edges repeated (Pillow's
    /// `MaxFilter`, `MinFilter`).
    pub fn rank(&self, size: usize, max: bool) -> Image {
        let r = (size / 2) as i64;
        let (w, h) = (self.width as i64, self.height as i64);
        let pick = |a: u8, b: u8| if max { a.max(b) } else { a.min(b) };
        // (separable: the rows' extremes, then the columns')
        let mut rows = self.clone();
        for y in 0..h {
            for x in 0..w {
                let mut v = self.at(x as usize, y as usize);
                for dx in -r..=r {
                    v = pick(v, self.at((x + dx).clamp(0, w - 1) as usize, y as usize));
                }
                rows.data[(y * w + x) as usize] = v;
            }
        }
        let mut out = rows.clone();
        for y in 0..h {
            for x in 0..w {
                let mut v = rows.at(x as usize, y as usize);
                for dy in -r..=r {
                    v = pick(v, rows.at(x as usize, (y + dy).clamp(0, h - 1) as usize));
                }
                out.data[(y * w + x) as usize] = v;
            }
        }
        out
    }

    /// The region of pixels equal to the one at (x, y) and joined to it side by side, set to `value` (Pillow's
    /// `ImageDraw.floodfill` of a grey image).
    pub fn flood(&mut self, x: usize, y: usize, value: u8) {
        let old = self.at(x, y);
        if old == value {
            return;
        }
        let mut todo = vec![(x, y)];
        self.data[y * self.width + x] = value;
        while let Some((x, y)) = todo.pop() {
            let mut visit = |x: usize, y: usize, data: &mut Vec<u8>| {
                if data[y * self.width + x] == old {
                    data[y * self.width + x] = value;
                    todo.push((x, y));
                }
            };
            if x > 0 {
                visit(x - 1, y, &mut self.data);
            }
            if x + 1 < self.width {
                visit(x + 1, y, &mut self.data);
            }
            if y > 0 {
                visit(x, y - 1, &mut self.data);
            }
            if y + 1 < self.height {
                visit(x, y + 1, &mut self.data);
            }
        }
    }

    /// Blurred as Pillow's `GaussianBlur(radius)`: three box blurs a direction, of the radius that gives that
    /// Gaussian's spread.
    pub fn gaussian_blur(&self, radius: f64) -> Image {
        let passes = 3.0;
        let sigma2 = radius * radius / passes;
        let big_l = (12.0 * sigma2 + 1.0).sqrt();
        let l = ((big_l - 1.0) / 2.0).floor();
        let a = (2.0 * l + 1.0) * (l * (l + 1.0) - 3.0 * sigma2) / (6.0 * (sigma2 - (l + 1.0) * (l + 1.0)));
        let r = (l + a) as f32;
        let mut im = self.clone();
        for _ in 0..3 {
            im = im.box_blur_rows(r);
        }
        let mut t = im.transposed();
        for _ in 0..3 {
            t = t.box_blur_rows(r);
        }
        t.transposed()
    }

    fn transposed(&self) -> Image {
        let c = self.channels;
        let mut out = Image { width: self.height, height: self.width, channels: c, data: vec![0; self.data.len()] };
        for y in 0..self.height {
            for x in 0..self.width {
                let s = (y * self.width + x) * c;
                let d = (x * self.height + y) * c;
                out.data[d..d + c].copy_from_slice(&self.data[s..s + c]);
            }
        }
        out
    }

    /// Each row blurred by a box of radius `r` (whole pixels and a fraction), the edges repeated (Pillow's
    /// `ImagingLineBoxBlur`).
    fn box_blur_rows(&self, r: f32) -> Image {
        let c = self.channels;
        let w = self.width;
        let radius = r as usize;
        let ww = ((1u64 << 24) as f32 / (r * 2.0 + 1.0)) as u64;
        let fw = ((1u64 << 24) - (radius as u64 * 2 + 1) * ww) / 2;
        let mut out = self.clone();
        let last = w as i64 - 1;
        for y in 0..self.height {
            let row = &self.data[y * w * c..(y + 1) * w * c];
            let at = |x: i64, ch: usize| u64::from(row[x.clamp(0, last) as usize * c + ch]);
            for ch in 0..c {
                let mut acc: u64 = (-(radius as i64)..=radius as i64).map(|x| at(x, ch)).sum();
                for x in 0..w as i64 {
                    let edges = at(x - radius as i64 - 1, ch) + at(x + radius as i64 + 1, ch);
                    let v = (acc * ww + edges * fw + (1 << 23)) >> 24;
                    out.data[(y * w + x as usize) * c + ch] = v.min(255) as u8;
                    acc = acc + at(x + radius as i64 + 1, ch) - at(x - radius as i64, ch);
                }
            }
        }
        out
    }

    /// The image as a PNG file.
    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, self.width as u32, self.height as u32);
            e.set_color(if self.channels == 4 { png::ColorType::Rgba } else { png::ColorType::Grayscale });
            e.set_depth(png::BitDepth::Eight);
            e.set_compression(png::Compression::High);
            let mut w = e.write_header().expect("a PNG header in memory");
            w.write_image_data(&self.data).expect("a PNG in memory");
        }
        out
    }

    /// A PNG file's image, as grey (`channels` 1) or RGBA (4). `what` names it in an error.
    pub fn from_png(bytes: &[u8], channels: usize, what: &str) -> Result<Image> {
        let bad = |e: &dyn std::fmt::Display| Error::format(what, format!("not a PNG this reads ({e})"));
        let mut d = png::Decoder::new(std::io::Cursor::new(bytes));
        d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut r = d.read_info().map_err(|e| bad(&e))?;
        // (a header may say any size: no picture this reads is larger than 4096 x 4096)
        let (iw, ih) = (r.info().width, r.info().height);
        if u64::from(iw) * u64::from(ih) > PICTURE_MOST {
            return Err(bad(&format!("{iw} x {ih}, larger than 4096 x 4096")));
        }
        let mut buf = vec![0; r.output_buffer_size().ok_or_else(|| bad(&"too large"))?];
        let info = r.next_frame(&mut buf).map_err(|e| bad(&e))?;
        let (w, h) = (info.width as usize, info.height as usize);
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
            png::ColorType::Rgb => {
                buf[..w * h * 3].as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
            }
            png::ColorType::GrayscaleAlpha => {
                buf[..w * h * 2].as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
            }
            png::ColorType::Grayscale => {
                let grey = Image { width: w, height: h, channels: 1, data: buf[..w * h].to_vec() };
                return Ok(if channels == 1 { grey } else { grey.to_rgba() });
            }
            png::ColorType::Indexed => return Err(bad(&"a palette left after expansion")),
        };
        let im = Image { width: w, height: h, channels: 4, data: rgba };
        Ok(if channels == 1 { im.to_grey() } else { im })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_that_says_it_is_huge_is_refused_before_its_buffer() {
        // (a header of 60,000 x 60,000 RGBA, 14 GB, and no data)
        let mut png = Image::rgba(1, 1).png();
        png[16..20].copy_from_slice(&60_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&60_000u32.to_be_bytes());
        let crc = crc32fast::hash(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let started = std::time::Instant::now();
        assert!(Image::from_png(&png, 4, "test").is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    fn noise(w: usize, h: usize, c: usize, seed: u32) -> Image {
        let mut s = seed;
        let data = (0..w * h * c)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 24) as u8
            })
            .collect();
        Image { width: w, height: h, channels: c, data }
    }

    #[test]
    fn a_flat_image_resizes_flat_and_a_resize_is_deterministic() {
        let flat = Image::new(37, 23, 4, &[10, 200, 30, 255]);
        for f in [Filter::Box, Filter::Bicubic, Filter::Lanczos] {
            let r = flat.resize(11, 50, f);
            assert!(r.data.as_chunks::<4>().0.iter().all(|p| *p == [10, 200, 30, 255]), "{f:?}");
        }
        let n = noise(40, 30, 1, 7);
        assert_eq!(n.resize(13, 9, Filter::Lanczos), n.resize(13, 9, Filter::Lanczos));
    }

    #[test]
    fn paste_rank_flood_and_png() {
        let mut im = Image::grey(5, 5);
        let mask = Image::new(3, 3, 1, &[128]);
        im.paste_colour(&[255], &mask, 1, 1);
        assert_eq!(im.at(2, 2), 128);
        assert_eq!(im.at(0, 0), 0);
        let grown = im.rank(3, true);
        assert_eq!(grown.at(0, 0), 128);
        assert_eq!(im.rank(3, false).at(2, 2), 128);
        let mut f = im.clone();
        f.flood(0, 0, 9);
        assert_eq!(f.at(4, 4), 9);
        assert_eq!(f.at(2, 2), 128);
        assert_eq!(im.bbox(), Some((1, 1, 4, 4)));
        let back = Image::from_png(&im.png(), 1, "t").unwrap();
        assert_eq!(back, im);
        let rgba = noise(4, 3, 4, 3);
        assert_eq!(Image::from_png(&rgba.png(), 4, "t").unwrap(), rgba);
    }

    #[test]
    fn a_blur_keeps_a_flat_image() {
        let flat = Image::new(30, 20, 1, &[77]);
        assert_eq!(flat.gaussian_blur(4.0), flat);
    }
}

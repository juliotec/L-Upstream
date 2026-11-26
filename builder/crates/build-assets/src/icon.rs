//! The icons: the window's (`longshot.png`, Linux: `host/posix/display_sdl.c`), and the files Windows takes an exe's
//! icon and name from (an `.ico`, a `.res` with the icon's images and the version strings Task Manager and the
//! taskbar show).
//!
//! The window's icon is the game's own (the .nup's JPEG, which the game-files stage reads), fitted in a 256-pixel
//! transparent square; without it, Longshot's (`assets/longshot.png`). An `.ico` here holds PNG images, as Windows
//! Vista and later read them.

use crate::raster::{Filter, Image};
use build_core::{Error, Result, res};

/// The sizes an `.ico` holds.
pub const SIZES: [usize; 7] = [16, 24, 32, 48, 64, 128, 256];
/// The window icon's file, in the game folder.
pub const WINDOW_ICON: &str = "longshot.png";

/// A picture's bytes (PNG or JPEG) as RGBA. `what` names it in an error.
pub fn decode(bytes: &[u8], what: &str) -> Result<Image> {
    if bytes.starts_with(b"\x89PNG") {
        return Image::from_png(bytes, 4, what);
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        use zune_jpeg::zune_core::colorspace::ColorSpace;
        use zune_jpeg::zune_core::options::DecoderOptions;
        // (a header may say any size: no picture this reads is larger than 4096 x 4096)
        let options = DecoderOptions::default()
            .jpeg_set_out_colorspace(ColorSpace::RGBA)
            .set_max_width(4096)
            .set_max_height(4096);
        let mut d = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(bytes), options);
        let data = d.decode().map_err(|e| Error::format(what, format!("not a JPEG this reads ({e:?})")))?;
        let info = d.info().ok_or_else(|| Error::format(what, "a JPEG without its size"))?;
        return Ok(Image { width: usize::from(info.width), height: usize::from(info.height), channels: 4, data });
    }
    Err(Error::format(what, "neither a PNG nor a JPEG"))
}

/// Python's `round`: to the nearest, a half to the even one.
pub fn round_even(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// `im` fitted in a transparent `size` square, not stretched (Lanczos, centred).
pub fn square(im: &Image, size: usize) -> Image {
    let im = im.to_rgba();
    let k = size as f64 / im.width.max(im.height) as f64;
    let (w, h) = (round_even(im.width as f64 * k).max(1) as usize, round_even(im.height as f64 * k).max(1) as usize);
    let fit = im.resize(w, h, Filter::Lanczos);
    let mut out = Image::rgba(size, size);
    out.paste(&fit, &fit.channel(3), ((size - w) / 2) as i64, ((size - h) / 2) as i64);
    out
}

/// The window's icon: the game's picture when there is one that reads, else `fallback` (Longshot's PNG), as it is.
/// `None`: neither (the game has no icon).
pub fn window_icon(game: Option<&[u8]>, fallback: Option<&[u8]>) -> Option<Vec<u8>> {
    if let Some(im) = game.and_then(|b| decode(b, "the game's icon").ok()) {
        return Some(square(&im, 256).png());
    }
    fallback.map(<[u8]>::to_vec)
}

/// The images of an `.ico`: (size, PNG bytes) at each of [`SIZES`].
pub fn images_of(im: &Image) -> Vec<(u32, Vec<u8>)> {
    SIZES.iter().map(|&s| (s as u32, square(im, s).png())).collect()
}

/// An `.ico`'s images, when they are PNGs.
pub use build_core::res::ico_images as images_of_ico;
/// An `.ico` of PNG images.
pub use build_core::res::ico_of;

/// A `.res` file: the icon's images (when given) and the version strings (when given).
pub fn res_of(images: &[(u32, Vec<u8>)], strings: &[(&str, &str)]) -> Vec<u8> {
    let mut out = res::start(res::US_ENGLISH);
    if !images.is_empty() {
        out.extend(res::icon_entries(images));
    }
    if !strings.is_empty() {
        out.extend(res::version_entry(strings));
    }
    out
}

/// The version strings of a game named `title`: the exe's name in Windows (its icon stays Longshot's).
pub fn title_res(title: &str) -> Vec<u8> {
    res_of(
        &[],
        &[
            ("FileDescription", title),
            ("ProductName", title),
            ("InternalName", "longshot"),
            ("OriginalFilename", "longshot.exe"),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_jpeg_that_says_it_is_huge_is_refused() {
        // (SOI, then a baseline frame header of 60,000 x 60,000)
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 17, 8];
        j.extend_from_slice(&60_000u16.to_be_bytes());
        j.extend_from_slice(&60_000u16.to_be_bytes());
        j.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1, 0xFF, 0xD9]);
        assert!(decode(&j, "test").is_err());
    }

    #[test]
    fn a_picture_fits_its_square_and_an_ico_reads_back() {
        let wide_im = Image::new(40, 20, 4, &[200, 10, 10, 255]);
        let sq = square(&wide_im, 16);
        assert_eq!(sq.size(), (16, 16));
        assert_eq!(sq.px(8, 8), [200, 10, 10, 255]);
        assert_eq!(sq.px(8, 1)[3], 0, "transparent above the picture");
        let images = images_of(&wide_im);
        let ico = ico_of(&images);
        assert_eq!(images_of_ico(&ico).unwrap(), images);
        assert!(images_of_ico(b"\0\0\x02\0\x01\0").is_err());
        let back = decode(&images[6].1, "t").unwrap();
        assert_eq!(back.size(), (256, 256));
    }

    #[test]
    fn a_res_is_whole_entries() {
        let res = res_of(&images_of(&Image::new(4, 4, 4, &[1, 2, 3, 255])), &[("ProductName", "Game")]);
        // (each entry: its data size, its header size, the data padded to 4; build_core::res checks the format)
        let mut at = 0;
        let mut kinds = Vec::new();
        while at < res.len() {
            let size = u32::from_le_bytes(res[at..at + 4].try_into().unwrap()) as usize;
            let head = u32::from_le_bytes(res[at + 4..at + 8].try_into().unwrap()) as usize;
            kinds.push(u16::from_le_bytes([res[at + 10], res[at + 11]]));
            at += head + size + (size.wrapping_neg() & 3);
        }
        assert_eq!(at, res.len());
        assert_eq!(kinds, [0, 3, 3, 3, 3, 3, 3, 3, 14, 16]);
        assert!(title_res("Game").len() > 100);
        assert_eq!(window_icon(None, Some(b"x")), Some(b"x".to_vec()));
        assert_eq!(window_icon(Some(b"bad"), None), None);
    }
}

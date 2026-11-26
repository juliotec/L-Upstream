//! The tests' fixtures: synthetic text (words, a run of zeros, noise), and frames the zstd command made of it.
//! (text.dict and text.dict.zst: a 2 KB dictionary trained on it and a frame of its first 20,000 bytes made with it,
//! by python-zstandard 0.25; host/zs.c's test reads them, tests/zs_test.c.)

pub(crate) fn text() -> &'static [u8] {
    include_bytes!("../testdata/text.raw")
}

/// `zstd -1` and `zstd --ultra -22 --no-check`.
pub(crate) const FRAMES: [&[u8]; 2] =
    [include_bytes!("../testdata/text.1.zst"), include_bytes!("../testdata/text.22.zst")];

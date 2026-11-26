//! The game's `.zs` files: zstd frames, most made with one of the dictionaries in `Pack/ZsDic.pack.zs` (a SARC of
//! them, itself a frame made without one). A frame names its dictionary by id, so a decoder holding all of them
//! reads any of the game's files.
//!
//! The files the build writes for the game (the reworded texts) are frames made without a dictionary, with the content
//! size in their header: the game sizes its buffer by it.

use crate::romfs::Romfs;
use crate::sarc;
use build_core::{Error, Result};
use ruzstd::decoding::{BlockDecodingStrategy, Dictionary, FrameDecoder};

/// The dictionary pack, under the romfs.
pub const DICTIONARY_PACK: &str = "Pack/ZsDic.pack.zs";

/// The largest file this reads unpacked (the biggest of the game's is 244 MB).
const MOST: usize = 512 << 20;
/// The most reserved at once for a frame's stated size: a header may state any size, so no more than 64 times the
/// frame's own bytes, nor 256 MB (what is larger grows as it unpacks).
const RESERVE_MOST: usize = 256 << 20;

/// What to reserve for a frame that states `stated` bytes, `frame` bytes of it in hand.
fn reserve_for(stated: usize, frame: usize) -> usize {
    stated.min(MOST).min(frame.saturating_mul(64)).min(RESERVE_MOST)
}

/// zstd's frame magic.
pub const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// The game's dictionaries, as read from the pack.
#[derive(Clone, Debug)]
pub struct Dictionaries {
    files: Vec<(Option<String>, Vec<u8>)>,
}

impl Dictionaries {
    /// The dictionaries of `romfs`'s pack.
    pub fn load(romfs: &dyn Romfs) -> Result<Dictionaries> {
        Dictionaries::from_pack(&romfs.read(DICTIONARY_PACK)?)
    }

    /// The dictionaries of a pack's bytes (the zstd frame of a SARC).
    pub fn from_pack(packed: &[u8]) -> Result<Dictionaries> {
        let pack = Decoder::plain().decompress(packed, DICTIONARY_PACK)?;
        let files =
            sarc::entries(&pack, DICTIONARY_PACK)?.into_iter().map(|e| (e.name, pack[e.range].to_vec())).collect();
        Ok(Dictionaries { files })
    }

    /// The dictionary file named `name` (`zs.zsdic`, the one most of the game's files use).
    pub fn named(&self, name: &str) -> Option<&[u8]> {
        self.files.iter().find(|(n, _)| n.as_deref() == Some(name)).map(|(_, b)| b.as_slice())
    }

    /// A decoder that reads frames made with any of these (or with none).
    pub fn decoder(&self) -> Decoder {
        let mut d = Decoder::plain();
        for (_, raw) in &self.files {
            // (a file of the pack that is not a dictionary is not one any frame can name)
            if let Ok(dict) = Dictionary::decode_dict(raw) {
                let _ = d.inner.add_dict(dict);
            }
        }
        d
    }

    /// The romfs file `rel` unpacked (or its bytes as they are, when it is not zstd).
    pub fn read(&self, romfs: &dyn Romfs, rel: &str) -> Result<Vec<u8>> {
        self.decoder().unpack(&romfs.read(rel)?, rel)
    }
}

/// A zstd decoder, holding the dictionaries frames may name. One per thread (it keeps its tables between frames).
pub struct Decoder {
    inner: FrameDecoder,
}

impl Decoder {
    /// A decoder with no dictionaries.
    pub fn plain() -> Decoder {
        Decoder { inner: FrameDecoder::new() }
    }

    /// `data` unpacked when it is zstd, else as it is. `what` names it in an error.
    pub fn unpack(&mut self, data: &[u8], what: &str) -> Result<Vec<u8>> {
        if data.starts_with(&MAGIC) { self.decompress(data, what) } else { Ok(data.to_vec()) }
    }

    /// Every frame of `data`, unpacked one after the other. `what` names it in an error.
    pub fn decompress(&mut self, data: &[u8], what: &str) -> Result<Vec<u8>> {
        let bad = |e: &dyn std::fmt::Display| Error::format(what, format!("zstd: {e}"));
        let mut input = data;
        let mut out = Vec::new();
        while !input.is_empty() {
            self.inner.init(&mut input).map_err(|e| bad(&e))?;
            // (the stated size, when there is one: one allocation, not a buffer grown a piece at a time)
            let stated = usize::try_from(self.inner.content_size()).unwrap_or(MOST).min(MOST);
            out.reserve(reserve_for(stated, data.len()));
            loop {
                self.inner.decode_blocks(&mut input, BlockDecodingStrategy::UptoBytes(4 << 20)).map_err(|e| bad(&e))?;
                self.inner.collect_to_writer(&mut out).map_err(|e| bad(&e))?;
                if out.len() > MOST {
                    return Err(Error::format(what, "unpacks to more than 512 MB"));
                }
                if self.inner.is_finished() {
                    break;
                }
            }
            self.inner.collect_to_writer(&mut out).map_err(|e| bad(&e))?;
        }
        Ok(out)
    }
}

/// `data` as one zstd frame made without a dictionary, its content size in the header.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let frame = ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest);
    with_content_size(&frame, data.len() as u64)
}

/// A frame with its content size put in its header: the encoder leaves it out, and the game sizes its buffer by it.
/// The header is the magic, the descriptor byte, then (single segment unset) the window byte, the dictionary id and
/// the content size, each as the descriptor says; the blocks and the checksum after it are kept as they are.
fn with_content_size(frame: &[u8], size: u64) -> Vec<u8> {
    let descriptor = frame[4];
    let single = descriptor & 0x20 != 0;
    let dict_bytes = [0usize, 1, 2, 4][usize::from(descriptor & 3)];
    let fcs_bytes = match descriptor >> 6 {
        0 if single => 1,
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let start = 5 + usize::from(!single) + dict_bytes;
    let mut out = Vec::with_capacity(frame.len() + 8);
    out.extend_from_slice(&frame[..4]);
    // (an 8-byte size, flag 3: right for any length; single segment kept as it was, so the window byte stays)
    out.push(descriptor | 0xC0);
    out.extend_from_slice(&frame[5..start]);
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&frame[start + fcs_bytes..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_that_states_a_huge_size_reserves_what_its_bytes_can_hold() {
        // (a frame of 20 bytes stating 2 GB: before, 2 GB were reserved at once)
        assert_eq!(reserve_for(2 << 30, 20), 20 * 64);
        assert_eq!(reserve_for(1000, 20), 1000);
        assert_eq!(reserve_for(400 << 20, 100 << 20), RESERVE_MOST);
        assert_eq!(reserve_for(usize::MAX, usize::MAX), RESERVE_MOST);
    }

    #[test]
    fn a_frame_states_its_size_and_reads_back() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8 ^ (i / 1000) as u8).collect();
        let frame = compress(&data);
        let mut d = FrameDecoder::new();
        let mut input = frame.as_slice();
        d.init(&mut input).unwrap();
        assert_eq!(d.content_size(), data.len() as u64);
        assert_eq!(Decoder::plain().decompress(&frame, "test").unwrap(), data);
        assert_eq!(compress(&data), frame, "the same bytes every time");
        assert_eq!(Decoder::plain().unpack(b"plain", "test").unwrap(), b"plain");
    }

    #[test]
    fn the_empty_file() {
        assert_eq!(Decoder::plain().decompress(&compress(b""), "test").unwrap(), b"");
    }
}

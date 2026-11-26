//! The logical instructions' bitmask immediates (the Arm ARM's DecodeBitMasks, for the immediate forms).

/// The value of N:immr:imms in a register of `width` bits (32 or 64), or `None` when the fields name no bitmask (the
/// encoding is unallocated).
pub fn decode_bit_masks(n: bool, imms: u32, immr: u32, width: u32) -> Option<u64> {
    let combined = ((n as u32) << 6) | (!imms & 0x3f);
    if combined == 0 {
        return None;
    }
    let len = 31 - combined.leading_zeros();
    if len < 1 || (1u32 << len) > width {
        return None;
    }
    let levels = (1u32 << len) - 1;
    if imms & levels == levels {
        return None;
    }
    let s = imms & levels;
    let r = immr & levels;
    let esize = 1u32 << len;
    let welem: u64 = (1u64 << (s + 1)) - 1; // (s + 1 <= 63: s < levels)
    let emask: u64 = if esize == 64 { u64::MAX } else { (1u64 << esize) - 1 };
    let rotated = if r == 0 { welem } else { ((welem >> r) | (welem << (esize - r))) & emask };
    let mut value = 0u64;
    let mut i = 0;
    while i < width {
        value |= rotated << i;
        i += esize;
    }
    if width == 32 {
        value &= 0xffff_ffff;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::decode_bit_masks;

    #[test]
    fn known_values() {
        // and x0, x1, #0xff: N=1 immr=0 imms=7
        assert_eq!(decode_bit_masks(true, 7, 0, 64), Some(0xff));
        // and w0, w1, #0x3: N=0 immr=0 imms=1
        assert_eq!(decode_bit_masks(false, 1, 0, 32), Some(0x3));
        // 0x5555555555555555: element 2, one bit
        assert_eq!(decode_bit_masks(false, 0b111100, 0, 64), Some(0x5555_5555_5555_5555));
        // 0x8000000000000000: N=1 immr=1 imms=0
        assert_eq!(decode_bit_masks(true, 0, 1, 64), Some(0x8000_0000_0000_0000));
        // all ones is not a bitmask
        assert_eq!(decode_bit_masks(true, 63, 0, 64), None);
        // N=1 in a W register
        assert_eq!(decode_bit_masks(true, 7, 0, 32), None);
    }
}

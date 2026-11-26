//! The constants of the Arm ARM's reciprocal square root estimate and of AES, worked out from their definitions (the
//! reciprocal estimate needs no table: sem computes it, one division).

/// The AES S-box (FIPS 197): each byte's inverse in GF(2^8) (0 for 0), through the affine map.
pub(crate) fn aes_sbox() -> [u8; 256] {
    let mul = |mut a: u8, mut b: u8| {
        let mut p = 0u8;
        while b != 0 {
            if b & 1 == 1 {
                p ^= a;
            }
            let carry = a & 0x80 != 0;
            a <<= 1;
            if carry {
                a ^= 0x1b;
            }
            b >>= 1;
        }
        p
    };
    let mut sbox = [0u8; 256];
    for (x, out) in sbox.iter_mut().enumerate() {
        let x = x as u8;
        let inv = if x == 0 { 0 } else { (1..=255u8).find(|&y| mul(x, y) == 1).expect("an inverse") };
        let rot = |n: u32| inv.rotate_left(n);
        *out = inv ^ rot(1) ^ rot(2) ^ rot(3) ^ rot(4) ^ 0x63;
    }
    sbox
}

/// RecipSqrtEstimate (the Arm ARM's): for `a` of 128 to 511 (an operand in [0.25, 1) in units of 1/512), the estimate
/// of its reciprocal square root, 256 to 511.
pub(crate) fn recip_sqrt_estimate(a: u32) -> u32 {
    let a = if a < 256 { a * 2 + 1 } else { ((a >> 1) << 1 | 1) * 2 };
    let mut b = 512u64;
    while u64::from(a) * (b + 1) * (b + 1) < 1 << 28 {
        b += 1;
    }
    b.div_ceil(2) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values() {
        let s = aes_sbox();
        assert_eq!((s[0x00], s[0x01], s[0x53], s[0xff]), (0x63, 0x7c, 0xed, 0x16));
        assert!((128..512).all(|a| (256..512).contains(&recip_sqrt_estimate(a))));
    }
}

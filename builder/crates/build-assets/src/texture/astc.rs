//! ASTC, LDR, two-dimensional: a decoder of every block the format has, and an encoder of 4x4 blocks.
//!
//! The decoder follows the Khronos specification (the block modes, integer sequence encoding, the colour endpoint
//! modes, the partition function, the weights' infill). The encoder searches, for each block: a single colour (void
//! extent), one partition or two (the partition pattern nearest a two-way split of the block's colours), one plane
//! of weights or two (the second for one channel: alpha first), the endpoint mode the colours need (luminance,
//! luminance and alpha, RGB, RGBA), and every weight precision with the endpoints' precision that fits beside it;
//! the endpoints are fitted along the colours' principal axis, then refined by least squares on the quantised
//! weights. Each candidate is decoded and the one nearest the block (squared error over the channels) is kept. It is
//! integer and plain floating-point arithmetic only, the same on every PC, and blocks are independent (the caller may
//! run them on any number of threads).

/// The weight precisions: levels, by quant mode.
const WEIGHT_LEVELS: [u32; 12] = [2, 3, 4, 5, 6, 8, 10, 12, 16, 20, 24, 32];
/// The colour precisions: levels, by quant mode.
const COLOUR_LEVELS: [u32; 21] = [2, 3, 4, 5, 6, 8, 10, 12, 16, 20, 24, 32, 40, 48, 64, 80, 96, 128, 160, 192, 256];

/// A block's texels as RGBA.
pub type Texels = Vec<[u8; 4]>;

/// A level count's encoding: (trits, quints, bits a value).
fn ise_kind(levels: u32) -> (bool, bool, u32) {
    if levels.is_multiple_of(3) && (levels / 3).is_power_of_two() {
        (true, false, (levels / 3).trailing_zeros())
    } else if levels.is_multiple_of(5) && (levels / 5).is_power_of_two() {
        (false, true, (levels / 5).trailing_zeros())
    } else {
        (false, false, levels.trailing_zeros())
    }
}

/// The bits `count` values of `levels` take.
pub fn ise_bits(count: u32, levels: u32) -> u32 {
    let (t, q, m) = ise_kind(levels);
    count * m
        + if t {
            (8 * count).div_ceil(5)
        } else if q {
            (7 * count).div_ceil(3)
        } else {
            0
        }
}

/// A trit block's five trits.
fn trits_of(t: u32) -> [u32; 5] {
    let bit = |k: u32| (t >> k) & 1;
    let (c, t4, t3);
    if (t >> 2) & 7 == 7 {
        c = (t >> 5 & 7) << 2 | (t & 3);
        t4 = 2;
        t3 = 2;
    } else {
        c = t & 0x1F;
        if (t >> 5) & 3 == 3 {
            t4 = 2;
            t3 = bit(7);
        } else {
            t4 = bit(7);
            t3 = (t >> 5) & 3;
        }
    }
    let cb = |k: u32| (c >> k) & 1;
    let (t2, t1, t0);
    if c & 3 == 3 {
        t2 = 2;
        t1 = cb(4);
        t0 = cb(3) << 1 | (cb(2) & (cb(3) ^ 1));
    } else if (c >> 2) & 3 == 3 {
        t2 = 2;
        t1 = 2;
        t0 = c & 3;
    } else {
        t2 = cb(4);
        t1 = (c >> 2) & 3;
        t0 = cb(1) << 1 | (cb(0) & (cb(1) ^ 1));
    }
    [t0, t1, t2, t3, t4]
}

/// A quint block's three quints.
fn quints_of(q: u32) -> [u32; 3] {
    let bit = |k: u32| (q >> k) & 1;
    if (q >> 1) & 3 == 3 && (q >> 5) & 3 == 0 {
        let q2 = bit(0) << 2 | (bit(4) & (bit(0) ^ 1)) << 1 | (bit(3) & (bit(0) ^ 1));
        return [4, 4, q2];
    }
    let (q2, c) = if (q >> 1) & 3 == 3 {
        (4, (q >> 3 & 3) << 3 | ((!(q >> 5)) & 3) << 1 | bit(0))
    } else {
        ((q >> 5) & 3, q & 0x1F)
    };
    let (q1, q0) = if c & 7 == 5 { (4, (c >> 3) & 3) } else { ((c >> 3) & 3, c & 7) };
    [q0, q1, q2]
}

/// The encodings of trit and quint tuples: the first block value that decodes to each.
struct Packing {
    trits: [u8; 243],
    quints: [u8; 125],
}

fn packing() -> &'static Packing {
    static P: std::sync::OnceLock<Packing> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let mut p = Packing { trits: [0; 243], quints: [0; 125] };
        let mut seen = [false; 243];
        for t in 0..256u32 {
            let v = trits_of(t);
            let k = (v[0] + 3 * v[1] + 9 * v[2] + 27 * v[3] + 81 * v[4]) as usize;
            if !seen[k] {
                seen[k] = true;
                p.trits[k] = t as u8;
            }
        }
        let mut seen = [false; 125];
        for q in 0..128u32 {
            let v = quints_of(q);
            let k = (v[0] + 5 * v[1] + 25 * v[2]) as usize;
            if !seen[k] {
                seen[k] = true;
                p.quints[k] = q as u8;
            }
        }
        p
    })
}

/// A block's 128 bits, read and written a field at a time.
#[derive(Clone, Copy, Default)]
struct Bits(u128);

impl Bits {
    fn get(self, at: u32, n: u32) -> u32 {
        if n == 0 || at >= 128 {
            return 0;
        }
        ((self.0 >> at) & ((1u128 << n) - 1)) as u32
    }
    fn set(&mut self, at: u32, n: u32, v: u32) {
        if n > 0 && at < 128 {
            let mask = ((1u128 << n) - 1) << at;
            self.0 = (self.0 & !mask) | ((u128::from(v) << at) & mask);
        }
    }
}

/// `count` values of `levels` read from `bits` starting at `at`.
fn ise_decode(bits: Bits, at: u32, count: u32, levels: u32) -> Vec<u32> {
    let (t, q, m) = ise_kind(levels);
    let mut out = Vec::with_capacity(count as usize);
    let end = at + ise_bits(count, levels);
    let mut p = at;
    let mut take = |n: u32| {
        // (past the sequence's last bit: 0)
        let v = bits.get(p, n.min(end.saturating_sub(p)));
        p += n;
        v
    };
    if t {
        while (out.len() as u32) < count {
            let (mut lo, mut tb) = ([0u32; 5], 0u32);
            for (i, (shift, len)) in [(0, 2), (2, 2), (4, 1), (5, 2), (7, 1)].into_iter().enumerate() {
                lo[i] = take(m);
                tb |= take(len) << shift;
            }
            for (i, d) in trits_of(tb).into_iter().enumerate() {
                out.push(d << m | lo[i]);
            }
        }
    } else if q {
        while (out.len() as u32) < count {
            let (mut lo, mut qb) = ([0u32; 3], 0u32);
            for (i, (shift, len)) in [(0, 3), (3, 2), (5, 2)].into_iter().enumerate() {
                lo[i] = take(m);
                qb |= take(len) << shift;
            }
            for (i, d) in quints_of(qb).into_iter().enumerate() {
                out.push(d << m | lo[i]);
            }
        }
    } else {
        for _ in 0..count {
            out.push(take(m));
        }
    }
    out.truncate(count as usize);
    out
}

/// `values` of `levels` written into `bits` from `at` (the bits past `ise_bits` left as they are).
fn ise_encode(bits: &mut Bits, at: u32, values: &[u32], levels: u32) {
    let (t, q, m) = ise_kind(levels);
    let total = ise_bits(values.len() as u32, levels);
    let end = at + total;
    let mut p = at;
    let mut put = |n: u32, v: u32, bits: &mut Bits| {
        let n2 = n.min(end.saturating_sub(p));
        bits.set(p, n2, v);
        p += n;
    };
    let mask = (1u32 << m) - 1;
    if t {
        for chunk in values.chunks(5) {
            let mut v = [0u32; 5];
            v[..chunk.len()].copy_from_slice(chunk);
            let k = v.iter().rev().fold(0, |a, &x| a * 3 + (x >> m)) as usize;
            let tb = u32::from(packing().trits[k]);
            for (i, (shift, len)) in [(0, 2), (2, 2), (4, 1), (5, 2), (7, 1)].into_iter().enumerate() {
                put(m, v[i] & mask, bits);
                put(len, (tb >> shift) & ((1 << len) - 1), bits);
            }
        }
    } else if q {
        for chunk in values.chunks(3) {
            let mut v = [0u32; 3];
            v[..chunk.len()].copy_from_slice(chunk);
            let k = v.iter().rev().fold(0, |a, &x| a * 5 + (x >> m)) as usize;
            let qb = u32::from(packing().quints[k]);
            for (i, (shift, len)) in [(0, 3), (3, 2), (5, 2)].into_iter().enumerate() {
                put(m, v[i] & mask, bits);
                put(len, (qb >> shift) & ((1 << len) - 1), bits);
            }
        }
    } else {
        for &v in values {
            put(m, v, bits);
        }
    }
}

/// A colour value of `levels` unquantised to 0..255.
fn unquant_colour(v: u32, levels: u32) -> u32 {
    let (t, q, m) = ise_kind(levels);
    if !t && !q {
        // (the bits repeated to fill eight)
        let mut out = 0;
        let mut have = 0;
        while have < 8 {
            out = out << m | v;
            have += m;
        }
        return out >> (have - 8);
    }
    let d = v >> m;
    let bit = |k: u32| (v >> k) & 1;
    let a = if bit(0) == 1 { 0x1FF } else { 0 };
    let (b, c) = match (t, m) {
        (true, 1) => (0, 204),
        (true, 2) => (bit(1) * 0b1_0001_0110, 93),
        (true, 3) => {
            let cb = v >> 1 & 3;
            (cb << 7 | cb << 2 | cb, 44)
        }
        (true, 4) => {
            let dcb = v >> 1 & 7;
            (dcb << 6 | dcb, 22)
        }
        (true, 5) => {
            let edcb = v >> 1 & 15;
            (edcb << 5 | edcb >> 2, 11)
        }
        (true, _) => {
            let fedcb = v >> 1 & 31;
            (fedcb << 4 | fedcb >> 4, 5)
        }
        (false, 1) => (0, 113),
        (false, 2) => (bit(1) * 0b1_0000_1100, 54),
        (false, 3) => {
            let cb = v >> 1 & 3;
            (cb << 7 | cb << 1 | cb >> 1, 26)
        }
        (false, 4) => {
            let dcb = v >> 1 & 7;
            (dcb << 6 | dcb >> 1, 13)
        }
        (false, 5) => {
            let edcb = v >> 1 & 15;
            (edcb << 5 | edcb >> 3, 6)
        }
        (false, _) => {
            let fedcb = v >> 1 & 31;
            (fedcb << 4, 3)
        }
    };
    let mut x = d * c + b;
    x ^= a;
    (a & 0x80) | (x >> 2)
}

/// A weight of `levels` unquantised to 0..64.
fn unquant_weight(v: u32, levels: u32) -> u32 {
    let (t, q, m) = ise_kind(levels);
    let out = if !t && !q {
        let mut out = 0;
        let mut have = 0;
        while have < 6 {
            out = out << m | v;
            have += m;
        }
        out >> (have - 6)
    } else if m == 0 {
        if t { [0, 32, 63][v as usize] } else { [0, 16, 32, 47, 63][v as usize] }
    } else {
        let d = v >> m;
        let bit = |k: u32| (v >> k) & 1;
        let a = if bit(0) == 1 { 0x7F } else { 0 };
        let (b, c) = match (t, m) {
            (true, 1) => (0, 50),
            (true, 2) => (bit(1) * 0b100_0101, 23),
            (true, _) => {
                let cb = v >> 1 & 3;
                (cb << 5 | cb, 11)
            }
            (false, 1) => (0, 28),
            (false, _) => (bit(1) * 0b100_0010, 13),
        };
        let mut x = d * c + b;
        x ^= a;
        (a & 0x20) | (x >> 2)
    };
    if out > 32 { out + 1 } else { out }
}

fn hash52(mut p: u32) -> u32 {
    p ^= p >> 15;
    p = p.wrapping_mul(0xEEDE_0891);
    p ^= p >> 5;
    p = p.wrapping_add(p << 16);
    p ^= p >> 7;
    p ^= p >> 3;
    p ^= p << 6;
    p ^= p >> 17;
    p
}

/// The partition of texel (x, y) in a block of `partitions` by `seed` (the specification's function, 2D).
fn select_partition(seed: u32, x: u32, y: u32, partitions: u32, small: bool) -> u32 {
    let (x, y) = if small { (x << 1, y << 1) } else { (x, y) };
    let seed = seed + (partitions - 1) * 1024;
    let r = hash52(seed);
    let mut s = [
        r & 0xF,
        (r >> 4) & 0xF,
        (r >> 8) & 0xF,
        (r >> 12) & 0xF,
        (r >> 16) & 0xF,
        (r >> 20) & 0xF,
        (r >> 24) & 0xF,
        (r >> 28) & 0xF,
        (r >> 18) & 0xF,
        (r >> 22) & 0xF,
        (r >> 26) & 0xF,
        r.rotate_left(2) & 0xF,
    ];
    for v in &mut s {
        *v *= *v;
    }
    let (sh1, sh2) = if seed & 1 != 0 {
        (if seed & 2 != 0 { 4 } else { 5 }, if partitions == 3 { 6 } else { 5 })
    } else {
        (if partitions == 3 { 6 } else { 5 }, if seed & 2 != 0 { 4 } else { 5 })
    };
    let sh3 = if seed & 0x10 != 0 { sh1 } else { sh2 };
    for (i, v) in s.iter_mut().enumerate() {
        *v >>= match i {
            0 | 2 | 4 | 6 => sh1,
            1 | 3 | 5 | 7 => sh2,
            _ => sh3,
        };
    }
    let z = 0;
    let a = (s[0] * x + s[1] * y + s[10] * z + (r >> 14)) & 0x3F;
    let b = (s[2] * x + s[3] * y + s[11] * z + (r >> 10)) & 0x3F;
    let mut c = (s[4] * x + s[5] * y + s[8] * z + (r >> 6)) & 0x3F;
    let mut d = (s[6] * x + s[7] * y + s[9] * z + (r >> 2)) & 0x3F;
    if partitions < 4 {
        d = 0;
    }
    if partitions < 3 {
        c = 0;
    }
    if a >= b && a >= c && a >= d {
        0
    } else if b >= c && b >= d {
        1
    } else if c >= d {
        2
    } else {
        3
    }
}

/// The colour of a block that does not decode.
const ERROR: [u8; 4] = [255, 0, 255, 255];

fn clamp8(v: i32) -> i32 {
    v.clamp(0, 255)
}

fn bit_transfer_signed(a: &mut i32, b: &mut i32) {
    *b >>= 1;
    *b |= *a & 0x80;
    *a >>= 1;
    *a &= 0x3F;
    if *a & 0x20 != 0 {
        *a -= 0x40;
    }
}

fn blue_contract(c: [i32; 4]) -> [i32; 4] {
    [(c[0] + c[2]) >> 1, (c[1] + c[2]) >> 1, c[2], c[3]]
}

/// A colour endpoint mode's two endpoints from its values; `None` for an HDR mode.
fn endpoints(cem: u32, v: &[u32]) -> Option<([i32; 4], [i32; 4])> {
    let mut v: Vec<i32> = v.iter().map(|&x| x as i32).collect();
    Some(match cem {
        0 => ([v[0], v[0], v[0], 255], [v[1], v[1], v[1], 255]),
        1 => {
            let l0 = (v[0] >> 2) | (v[1] & 0xC0);
            let l1 = (l0 + (v[1] & 0x3F)).min(255);
            ([l0, l0, l0, 255], [l1, l1, l1, 255])
        }
        4 => ([v[0], v[0], v[0], v[2]], [v[1], v[1], v[1], v[3]]),
        5 => {
            let (mut a1, mut b0, mut a3, mut b2) = (v[1], v[0], v[3], v[2]);
            bit_transfer_signed(&mut a1, &mut b0);
            bit_transfer_signed(&mut a3, &mut b2);
            let l = clamp8(b0 + a1);
            let a = clamp8(b2 + a3);
            ([b0, b0, b0, b2], [l, l, l, a])
        }
        6 => ([(v[0] * v[3]) >> 8, (v[1] * v[3]) >> 8, (v[2] * v[3]) >> 8, 255], [v[0], v[1], v[2], 255]),
        8 | 12 => {
            let (a0, a1) = if cem == 12 { (v[6], v[7]) } else { (255, 255) };
            if v[1] + v[3] + v[5] >= v[0] + v[2] + v[4] {
                ([v[0], v[2], v[4], a0], [v[1], v[3], v[5], a1])
            } else {
                (blue_contract([v[1], v[3], v[5], a1]), blue_contract([v[0], v[2], v[4], a0]))
            }
        }
        9 | 13 => {
            if cem == 9 {
                v.extend([255, 0]);
            }
            for k in [0usize, 2, 4, 6] {
                let (mut a, mut b) = (v[k + 1], v[k]);
                bit_transfer_signed(&mut a, &mut b);
                v[k + 1] = a;
                v[k] = b;
            }
            if cem == 9 {
                v[6] = 255;
                v[7] = 0;
            }
            let sum = |k: usize| clamp8(v[k] + v[k + 1]);
            if v[1] + v[3] + v[5] >= 0 {
                ([v[0], v[2], v[4], v[6]], [sum(0), sum(2), sum(4), sum(6)])
            } else {
                (blue_contract([sum(0), sum(2), sum(4), sum(6)]), blue_contract([v[0], v[2], v[4], v[6]]))
            }
        }
        10 => ([(v[0] * v[3]) >> 8, (v[1] * v[3]) >> 8, (v[2] * v[3]) >> 8, v[4]], [v[0], v[1], v[2], v[5]]),
        _ => return None,
    })
}

/// A decoded block's texels, `bw` x `bh`; sRGB: the endpoints widened as an sRGB texture's are.
pub fn decode_block(block: &[u8; 16], bw: u32, bh: u32, srgb: bool) -> Texels {
    decode_inner(block, bw, bh, srgb).unwrap_or_else(|| vec![ERROR; (bw * bh) as usize])
}

fn decode_inner(block: &[u8; 16], bw: u32, bh: u32, srgb: bool) -> Option<Texels> {
    let bits = Bits(u128::from_le_bytes(*block));
    let n = (bw * bh) as usize;
    let mode = bits.get(0, 11);
    if mode & 0x1FF == 0x1FC {
        // (one colour: four UNORM16 values; HDR ones not read)
        if mode & 0x200 != 0 {
            return None;
        }
        let c = [0, 1, 2, 3].map(|k| (bits.get(64 + 16 * k, 16) >> 8) as u8);
        return Some(vec![c; n]);
    }
    let (ws, wt, dual, wlevels) = block_mode(mode)?;
    if ws > bw || wt > bh {
        return None;
    }
    let wcount = ws * wt * (1 + u32::from(dual));
    let wbits = ise_bits(wcount, wlevels);
    let parts = bits.get(11, 2) + 1;
    if dual && parts == 4 {
        return None;
    }
    let (cems, colour_at, extra) = if parts == 1 {
        (vec![bits.get(13, 4)], 17, 0)
    } else {
        let field = bits.get(23, 6);
        if field & 3 == 0 {
            (vec![field >> 2; parts as usize], 29, 0)
        } else {
            let extra = 3 * parts - 4;
            let high = bits.get(128 - wbits - extra, extra);
            let all = field | high << 6;
            let base = (all & 3) - 1;
            let cems = (0..parts)
                .map(|i| {
                    let c = (all >> (2 + i)) & 1;
                    let m = (all >> (2 + parts + 2 * i)) & 3;
                    (base + c) << 2 | m
                })
                .collect();
            (cems, 29, extra)
        }
    };
    let ccs_bits = if dual { 2 } else { 0 };
    let ccs = if dual { bits.get(128 - wbits - extra - 2, 2) } else { 0 };
    let values: u32 = cems.iter().map(|c| 2 * ((c >> 2) + 1)).sum();
    if values > 18 {
        return None;
    }
    let avail = 128i32 - wbits as i32 - extra as i32 - ccs_bits - colour_at as i32;
    let clevels = *COLOUR_LEVELS.iter().rev().find(|&&l| ise_bits(values, l) as i32 <= avail)?;
    if clevels < 6 {
        return None;
    }
    let colours: Vec<u32> =
        ise_decode(bits, colour_at, values, clevels).into_iter().map(|v| unquant_colour(v, clevels)).collect();
    let mut eps = Vec::new();
    let mut k = 0;
    for &c in &cems {
        let len = (2 * ((c >> 2) + 1)) as usize;
        eps.push(endpoints(c, &colours[k..k + len])?);
        k += len;
    }
    // (the weights from the top of the block down)
    let rev = Bits(bits.0.reverse_bits());
    let raw: Vec<u32> = ise_decode(rev, 0, wcount, wlevels).into_iter().map(|v| unquant_weight(v, wlevels)).collect();
    let planes = 1 + usize::from(dual);
    let plane = |p: usize| -> Vec<u32> {
        let grid: Vec<u32> = raw.iter().skip(p).step_by(planes).copied().collect();
        infill(&grid, ws, wt, bw, bh)
    };
    let w0 = plane(0);
    let w1 = if dual { plane(1) } else { Vec::new() };
    let seed = bits.get(13, 10);
    let small = n < 31;
    let mut out = Vec::with_capacity(n);
    for y in 0..bh {
        for x in 0..bw {
            let i = (y * bw + x) as usize;
            let p = if parts == 1 { 0 } else { select_partition(seed, x, y, parts, small) as usize };
            let (e0, e1) = eps[p];
            let mut texel = [0u8; 4];
            for c in 0..4 {
                let w = if dual && c as u32 == ccs { w1[i] } else { w0[i] } as i32;
                let (c0, c1) = if srgb { (e0[c] << 8 | 0x80, e1[c] << 8 | 0x80) } else { (e0[c] * 257, e1[c] * 257) };
                texel[c] = (((c0 * (64 - w) + c1 * w + 32) >> 6) >> 8) as u8;
            }
            out.push(texel);
        }
    }
    Some(out)
}

/// A block mode's weight grid (width, height), whether it has two planes, and its weights' levels.
fn block_mode(mode: u32) -> Option<(u32, u32, bool, u32)> {
    let mut r = (mode >> 4) & 1;
    let (mut h, mut d) = ((mode >> 9) & 1, (mode >> 10) & 1);
    let a = (mode >> 5) & 3;
    let (ws, wt);
    if mode & 3 != 0 {
        r |= (mode & 3) << 1;
        let b = (mode >> 7) & 3;
        (ws, wt) = match (mode >> 2) & 3 {
            0 => (b + 4, a + 2),
            1 => (b + 8, a + 2),
            2 => (a + 2, b + 8),
            _ if mode & 0x100 != 0 => ((b & 1) + 2, a + 2),
            _ => (a + 2, (b & 1) + 6),
        };
    } else {
        r |= ((mode >> 2) & 3) << 1;
        if (mode >> 2) & 3 == 0 {
            return None;
        }
        let b = (mode >> 9) & 3;
        (ws, wt) = match (mode >> 7) & 3 {
            0 => (12, a + 2),
            1 => (a + 2, 12),
            2 => {
                d = 0;
                h = 0;
                (a + 6, b + 6)
            }
            _ => match a {
                0 => (6, 10),
                1 => (10, 6),
                _ => return None,
            },
        };
    }
    let levels = WEIGHT_LEVELS[(r as usize).checked_sub(2)? + 6 * h as usize];
    let count = ws * wt * (1 + d);
    let wbits = ise_bits(count, levels);
    (count <= 64 && (24..=96).contains(&wbits)).then_some((ws, wt, d != 0, levels))
}

/// A grid of weights (`ws` x `wt`) spread over a `bw` x `bh` block.
fn infill(grid: &[u32], ws: u32, wt: u32, bw: u32, bh: u32) -> Vec<u32> {
    if (ws, wt) == (bw, bh) {
        return grid.to_vec();
    }
    let ds = (1024 + bw / 2) / (bw - 1);
    let dt = (1024 + bh / 2) / (bh - 1);
    let mut out = Vec::with_capacity((bw * bh) as usize);
    let at = |s: u32, t: u32| grid[(t.min(wt - 1) * ws + s.min(ws - 1)) as usize];
    for t in 0..bh {
        for s in 0..bw {
            let gs = (ds * s * (ws - 1) + 32) >> 6;
            let gt = (dt * t * (wt - 1) + 32) >> 6;
            let (js, fs, jt, ft) = (gs >> 4, gs & 0xF, gt >> 4, gt & 0xF);
            let w11 = (fs * ft + 8) >> 4;
            let w10 = ft - w11;
            let w01 = fs - w11;
            let w00 = 16 + w11 - fs - ft; // (16 - fs - ft went below 0 before w11 was added: the same value, in order)
            let v = at(js, jt) * w00 + at(js + 1, jt) * w01 + at(js, jt + 1) * w10 + at(js + 1, jt + 1) * w11;
            out.push((v + 8) >> 4);
        }
    }
    out
}

// ---- the encoder (4x4 blocks)

/// A way to encode a block, before its endpoints and weights.
#[derive(Clone, Copy, Debug)]
struct Shape {
    /// 1 or 2.
    parts: u32,
    /// The partition seed (two partitions).
    seed: u32,
    /// The colour endpoint mode (0 L, 4 LA, 8 RGB, 12 RGBA), the same for every partition.
    cem: u32,
    /// The channel of the second plane of weights.
    plane2: Option<usize>,
}

fn cem_channels(cem: u32) -> &'static [usize] {
    match cem {
        0 => &[0],
        4 => &[0, 3],
        8 => &[0, 1, 2],
        _ => &[0, 1, 2, 3],
    }
}

/// The two partitions' patterns of every seed, as masks of the texels in partition 1.
fn patterns() -> &'static [u16; 1024] {
    static P: std::sync::OnceLock<[u16; 1024]> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let mut out = [0u16; 1024];
        for (seed, m) in out.iter_mut().enumerate() {
            for i in 0..16u32 {
                if select_partition(seed as u32, i % 4, i / 4, 2, true) == 1 {
                    *m |= 1 << i;
                }
            }
        }
        out
    })
}

/// The colour of a texel in a mode's channels, as floats (L the mean of R G B).
fn value(t: &[u8; 4], ch: usize, cem: u32) -> f64 {
    if ch == 0 && (cem == 0 || cem == 4) {
        (f64::from(t[0]) + f64::from(t[1]) + f64::from(t[2])) / 3.0
    } else {
        f64::from(t[ch])
    }
}

/// The endpoints along the principal axis of `pts` (each of `dims` values) and each point's place between them, 0
/// to 1.
fn fit_line(pts: &[Vec<f64>]) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let dims = pts.first().map_or(0, Vec::len);
    let n = pts.len() as f64;
    let mean: Vec<f64> = (0..dims).map(|d| pts.iter().map(|p| p[d]).sum::<f64>() / n).collect();
    // (the principal axis by power iteration on the covariance)
    let mut cov = vec![vec![0.0; dims]; dims];
    for p in pts {
        for i in 0..dims {
            for j in 0..dims {
                cov[i][j] += (p[i] - mean[i]) * (p[j] - mean[j]);
            }
        }
    }
    let mut axis: Vec<f64> = (0..dims).map(|i| cov[i][i].sqrt() + 1e-9).collect();
    for _ in 0..8 {
        let next: Vec<f64> = (0..dims).map(|i| (0..dims).map(|j| cov[i][j] * axis[j]).sum()).collect();
        let len = next.iter().map(|v| v * v).sum::<f64>().sqrt();
        if len < 1e-12 {
            break;
        }
        axis = next.iter().map(|v| v / len).collect();
    }
    let proj: Vec<f64> = pts.iter().map(|p| (0..dims).map(|d| (p[d] - mean[d]) * axis[d]).sum()).collect();
    let (lo, hi) = proj.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let e0: Vec<f64> = (0..dims).map(|d| mean[d] + lo * axis[d]).collect();
    let e1: Vec<f64> = (0..dims).map(|d| mean[d] + hi * axis[d]).collect();
    let span = hi - lo;
    let w = proj.iter().map(|&v| if span > 1e-9 { (v - lo) / span } else { 0.0 }).collect();
    (e0, e1, w)
}

/// The endpoints that best fit `pts` for the weights `w` (least squares), per dimension.
fn refit(pts: &[Vec<f64>], w: &[f64]) -> Option<(Vec<f64>, Vec<f64>)> {
    let dims = pts.first().map_or(0, Vec::len);
    let (mut aa, mut ab, mut bb) = (0.0, 0.0, 0.0);
    for &x in w {
        aa += (1.0 - x) * (1.0 - x);
        ab += (1.0 - x) * x;
        bb += x * x;
    }
    let det = aa * bb - ab * ab;
    if det.abs() < 1e-9 {
        return None;
    }
    let mut e0 = vec![0.0; dims];
    let mut e1 = vec![0.0; dims];
    for d in 0..dims {
        let (mut ra, mut rb) = (0.0, 0.0);
        for (p, &x) in pts.iter().zip(w) {
            ra += (1.0 - x) * p[d];
            rb += x * p[d];
        }
        e0[d] = (bb * ra - ab * rb) / det;
        e1[d] = (aa * rb - ab * ra) / det;
    }
    Some((e0, e1))
}

/// The level whose unquantised value is nearest each 0..255 value, for each colour precision; and each weight
/// precision's unquantised values.
struct Tables {
    colour: Vec<[u8; 256]>,
    weights: Vec<Vec<u32>>,
}

fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let colour = COLOUR_LEVELS
            .iter()
            .map(|&l| {
                let unq: Vec<u32> = (0..l).map(|v| unquant_colour(v, l)).collect();
                let mut t = [0u8; 256];
                for (x, slot) in t.iter_mut().enumerate() {
                    let best = (0..l)
                        .min_by_key(|&v| ((unq[v as usize] as i32 - x as i32).abs(), unq[v as usize]))
                        .unwrap_or(0);
                    *slot = best as u8;
                }
                t
            })
            .collect();
        let weights = WEIGHT_LEVELS.iter().map(|&l| (0..l).map(|v| unquant_weight(v, l)).collect()).collect();
        Tables { colour, weights }
    })
}

fn nearest_weight(w: f64, levels_at: usize) -> u32 {
    let unq = &tables().weights[levels_at];
    let target = w.clamp(0.0, 1.0) * 64.0;
    (0..unq.len() as u32)
        .min_by(|&a, &b| {
            let da = (f64::from(unq[a as usize]) - target).abs();
            let db = (f64::from(unq[b as usize]) - target).abs();
            da.total_cmp(&db)
        })
        .unwrap_or(0)
}

/// The squared error of a decoded block.
fn error(a: &[[u8; 4]], b: &[[u8; 4]]) -> u64 {
    a.iter().zip(b).map(|(x, y)| (0..4).map(|c| (i64::from(x[c]) - i64::from(y[c])).pow(2) as u64).sum::<u64>()).sum()
}

/// A void-extent block of one colour.
fn void_extent(c: [u8; 4]) -> [u8; 16] {
    let mut b = Bits(0);
    b.set(0, 12, 0xDFC); // (the mode, LDR, the two reserved ones)
    b.0 |= ((1u128 << 52) - 1) << 12; // (no extent given: all ones)
    for (k, &v) in c.iter().enumerate() {
        b.set(64 + 16 * k as u32, 16, u32::from(v) * 257);
    }
    b.0.to_le_bytes()
}

/// The block mode of a 4x4 grid of weights of `levels_at` (an index into the weight levels), dual or not.
fn mode_4x4(levels_at: usize, dual: bool) -> u32 {
    let (r, h) = ((levels_at % 6) as u32 + 2, (levels_at / 6) as u32);
    ((r >> 1) & 3) | (r & 1) << 4 | 2 << 5 | h << 9 | u32::from(dual) << 10
}

/// The block for `shape` at weight precision `wl` (an index) on `texels`, or `None` when it does not fit.
fn try_shape(texels: &[[u8; 4]], shape: Shape, wl: usize, srgb: bool) -> Option<([u8; 16], u64)> {
    let dual = shape.plane2.is_some();
    let wlevels = WEIGHT_LEVELS[wl];
    let wcount = 16 * (1 + u32::from(dual));
    let wbits = ise_bits(wcount, wlevels);
    if !(24..=96).contains(&wbits) {
        return None;
    }
    let chans = cem_channels(shape.cem);
    let values = shape.parts * chans.len() as u32 * 2;
    let colour_at = if shape.parts == 1 { 17 } else { 29 };
    let avail = 128 - wbits as i32 - if dual { 2 } else { 0 } - colour_at;
    let cl = COLOUR_LEVELS.iter().rposition(|&l| ise_bits(values, l) as i32 <= avail)?;
    let clevels = COLOUR_LEVELS[cl];
    if clevels < 6 {
        return None;
    }
    let part_of = |i: usize| -> usize {
        if shape.parts == 1 { 0 } else { usize::from(patterns()[shape.seed as usize] >> i & 1 == 1) }
    };
    let mut weights = [[0u32; 2]; 16];
    let mut colour_values = Vec::new();
    let qc = |v: f64| u32::from(tables().colour[cl][v.round().clamp(0.0, 255.0) as usize]);
    let uq = |q: u32| unquant_colour(q, clevels) as f64;
    for p in 0..shape.parts as usize {
        let idx: Vec<usize> = (0..16).filter(|&i| part_of(i) == p).collect();
        if idx.is_empty() {
            return None;
        }
        // (the channels on the first plane, and the second's)
        let first: Vec<usize> = chans.iter().copied().filter(|&c| Some(c) != shape.plane2).collect();
        let pts: Vec<Vec<f64>> =
            idx.iter().map(|&i| first.iter().map(|&c| value(&texels[i], c, shape.cem)).collect()).collect();
        let (mut e0, mut e1, mut w) = fit_line(&pts);
        let mut q0 = Vec::new();
        let mut q1 = Vec::new();
        for _round in 0..2 {
            q0 = e0.iter().map(|&v| qc(v)).collect();
            q1 = e1.iter().map(|&v| qc(v)).collect();
            // (each weight from the quantised endpoints)
            let (u0, u1): (Vec<f64>, Vec<f64>) =
                (q0.iter().map(|&q| uq(q)).collect(), q1.iter().map(|&q| uq(q)).collect());
            let dir: Vec<f64> = u0.iter().zip(&u1).map(|(a, b)| b - a).collect();
            let len2: f64 = dir.iter().map(|d| d * d).sum();
            w = pts
                .iter()
                .map(|p| {
                    if len2 > 0.0 {
                        p.iter().zip(&u0).zip(&dir).map(|((x, a), d)| (x - a) * d).sum::<f64>() / len2
                    } else {
                        0.0
                    }
                })
                .map(|x| f64::from(unquant_weight(nearest_weight(x, wl), wlevels)) / 64.0)
                .collect();
            match refit(&pts, &w) {
                Some((a, b)) => (e0, e1) = (a, b),
                None => break,
            }
        }
        for (k, &i) in idx.iter().enumerate() {
            weights[i][0] = nearest_weight(w[k], wl);
        }
        let (mut q0s, mut q1s) = (vec![0u32; 4], vec![0u32; 4]);
        for (k, &c) in first.iter().enumerate() {
            q0s[c] = q0[k];
            q1s[c] = q1[k];
        }
        if let Some(c2) = shape.plane2 {
            let vals: Vec<f64> = idx.iter().map(|&i| value(&texels[i], c2, shape.cem)).collect();
            let (lo, hi) = vals.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
            let (a, b) = (qc(lo), qc(hi));
            q0s[c2] = a;
            q1s[c2] = b;
            let (ua, ub) = (uq(a), uq(b));
            for (k, &i) in idx.iter().enumerate() {
                let x = if (ub - ua).abs() > 0.0 { (vals[k] - ua) / (ub - ua) } else { 0.0 };
                weights[i][1] = nearest_weight(x, wl);
            }
        }
        // (RGB modes: the second endpoint the larger sum, or the decoder swaps them and contracts blue)
        if (shape.cem == 8 || shape.cem == 12) && {
            let s = |q: &[u32]| q[..3].iter().map(|&v| unquant_colour(v, clevels)).sum::<u32>();
            s(&q1s) < s(&q0s)
        } {
            std::mem::swap(&mut q0s, &mut q1s);
            let top = WEIGHT_LEVELS[wl];
            for &i in &idx {
                for pl in 0..2 {
                    // (the weight that unquantises to 64 less this one's)
                    let unq = &tables().weights[wl];
                    let want = 64 - unq[weights[i][pl] as usize];
                    weights[i][pl] =
                        (0..top).min_by_key(|&v| (unq[v as usize] as i32 - want as i32).abs()).unwrap_or(0);
                }
            }
        }
        for &c in chans {
            colour_values.push(q0s[c]);
            colour_values.push(q1s[c]);
        }
    }
    let mut bits = Bits(0);
    bits.set(0, 11, mode_4x4(wl, dual));
    bits.set(11, 2, shape.parts - 1);
    if shape.parts == 1 {
        bits.set(13, 4, shape.cem);
    } else {
        bits.set(13, 10, shape.seed);
        bits.set(23, 6, shape.cem << 2);
    }
    ise_encode(&mut bits, colour_at as u32, &colour_values, clevels);
    if let Some(c2) = shape.plane2 {
        bits.set(128 - wbits - 2, 2, c2 as u32);
    }
    let flat: Vec<u32> = weights.iter().flat_map(|w| if dual { vec![w[0], w[1]] } else { vec![w[0]] }).collect();
    let mut rev = Bits(0);
    ise_encode(&mut rev, 0, &flat, wlevels);
    let mask = (1u128 << wbits) - 1;
    bits.0 = (bits.0 & !(mask << (128 - wbits))) | ((rev.0 & mask).reverse_bits());
    let block = bits.0.to_le_bytes();
    let err = error(&decode_block(&block, 4, 4, srgb), texels);
    Some((block, err))
}

/// The two-way split of the texels' colours (k-means, from the two farthest apart): partition 1's texels' mask.
fn split(texels: &[[u8; 4]]) -> u16 {
    let d = |a: &[u8; 4], b: &[u8; 4]| (0..4).map(|c| (i32::from(a[c]) - i32::from(b[c])).pow(2)).sum::<i32>();
    let mut best = (0, 0, -1);
    for i in 0..16 {
        for j in i + 1..16 {
            let v = d(&texels[i], &texels[j]);
            if v > best.2 {
                best = (i, j, v);
            }
        }
    }
    let mut centres = [texels[best.0].map(f64::from), texels[best.1].map(f64::from)];
    let mut mask = 0u16;
    for _ in 0..4 {
        mask = 0;
        let mut sums = [[0.0; 4]; 2];
        let mut counts = [0.0; 2];
        for (i, t) in texels.iter().enumerate() {
            let dist = |c: &[f64; 4]| (0..4).map(|k| (f64::from(t[k]) - c[k]).powi(2)).sum::<f64>();
            let p = usize::from(dist(&centres[1]) < dist(&centres[0]));
            if p == 1 {
                mask |= 1 << i;
            }
            counts[p] += 1.0;
            for k in 0..4 {
                sums[p][k] += f64::from(t[k]);
            }
        }
        for p in 0..2 {
            if counts[p] > 0.0 {
                centres[p] = [0, 1, 2, 3].map(|k| sums[p][k] / counts[p]);
            }
        }
    }
    mask
}

/// A 4x4 block of `texels` (row by row) encoded. sRGB: the error measured as an sRGB texture decodes.
pub fn encode_block(texels: &[[u8; 4]], srgb: bool) -> [u8; 16] {
    if texels.iter().all(|t| *t == texels[0]) {
        return void_extent(texels[0]);
    }
    let opaque = texels.iter().all(|t| t[3] == 255);
    let grey = texels.iter().all(|t| t[0] == t[1] && t[1] == t[2]);
    let cem = match (grey, opaque) {
        (true, true) => 0,
        (true, false) => 4,
        (false, true) => 8,
        (false, false) => 12,
    };
    let mut shapes = vec![Shape { parts: 1, seed: 0, cem, plane2: None }];
    let seconds: &[usize] = match cem {
        0 => &[],
        4 => &[3],
        8 => &[0, 1, 2],
        _ => &[3, 0, 1, 2],
    };
    for &c in seconds {
        shapes.push(Shape { parts: 1, seed: 0, cem, plane2: Some(c) });
    }
    let mask = split(texels);
    let mut seeds: Vec<(u32, u32)> = patterns()
        .iter()
        .enumerate()
        .filter(|(_, p)| **p != 0 && **p != 0xFFFF)
        .map(|(s, &p)| (s as u32, (p ^ mask).count_ones().min((p ^ !mask).count_ones())))
        .collect();
    seeds.sort_by_key(|&(s, d)| (d, s));
    for &(seed, _) in seeds.iter().take(4) {
        shapes.push(Shape { parts: 2, seed, cem, plane2: None });
    }
    let mut best: Option<([u8; 16], u64)> = None;
    for shape in shapes {
        for wl in 0..WEIGHT_LEVELS.len() {
            if let Some(found) = try_shape(texels, shape, wl, srgb)
                && best.is_none_or(|b| found.1 < b.1)
            {
                best = Some(found);
                if found.1 == 0 {
                    return found.0;
                }
            }
        }
    }
    best.map_or_else(|| void_extent(texels[0]), |b| b.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_sequences_round_trip() {
        for &levels in WEIGHT_LEVELS.iter().chain(COLOUR_LEVELS.iter()) {
            for count in [1u32, 2, 3, 5, 7, 16, 18].into_iter().filter(|&c| 5 + ise_bits(c, levels) <= 128) {
                let values: Vec<u32> = (0..count).map(|i| (i * 7 + 3) % levels).collect();
                let mut b = Bits(0);
                ise_encode(&mut b, 5, &values, levels);
                assert_eq!(ise_decode(b, 5, count, levels), values, "{levels} levels, {count} values");
            }
        }
    }

    #[test]
    fn unquantised_values_span_the_range() {
        for &l in COLOUR_LEVELS.iter().filter(|&&l| l >= 6) {
            let v: Vec<u32> = (0..l).map(|x| unquant_colour(x, l)).collect();
            assert_eq!(*v.iter().min().unwrap(), 0, "{l}");
            assert_eq!(*v.iter().max().unwrap(), 255, "{l}");
        }
        for &l in &WEIGHT_LEVELS {
            let v: Vec<u32> = (0..l).map(|x| unquant_weight(x, l)).collect();
            assert_eq!((*v.iter().min().unwrap(), *v.iter().max().unwrap()), (0, 64), "{l}");
        }
        // (the specification's table for 12 colour levels)
        let twelve: Vec<u32> = (0..12).map(|x| unquant_colour(x, 12)).collect();
        assert_eq!(twelve, [0, 255, 69, 186, 23, 232, 92, 163, 46, 209, 116, 139]);
    }

    fn block(f: impl Fn(usize) -> [u8; 4]) -> Texels {
        (0..16).map(f).collect()
    }

    #[test]
    fn encoded_blocks_decode_close() {
        let cases: Vec<Texels> = vec![
            block(|_| [10, 20, 30, 40]),
            block(|i| if i % 4 < 2 { [0, 0, 0, 0] } else { [255, 255, 255, 255] }),
            block(|i| [(i * 16) as u8, (i * 16) as u8, (i * 16) as u8, 255]),
            block(|i| [[0, 0, 0, 0], [0, 0, 0, 255], [255, 255, 255, 255], [128, 128, 128, 128]][i % 4]),
            block(|i| [(i * 13) as u8, (255 - i * 9) as u8, (i * 5) as u8, (i * 16) as u8]),
            block(|i| if (i % 4 + i / 4) % 2 == 0 { [200, 30, 30, 255] } else { [20, 40, 220, 255] }),
        ];
        for (k, t) in cases.iter().enumerate() {
            let b = encode_block(t, false);
            let back = decode_block(&b, 4, 4, false);
            let worst = t
                .iter()
                .zip(&back)
                .flat_map(|(a, b)| (0..4).map(move |c| (i32::from(a[c]) - i32::from(b[c])).abs()))
                .max()
                .unwrap();
            assert!(worst <= 24, "case {k}: off by {worst}");
            assert_eq!(encode_block(t, false), b, "the same block each time");
        }
        let two = encode_block(&cases[1], false);
        assert_eq!(decode_block(&two, 4, 4, false), cases[1], "two colours, exactly");
    }

    #[test]
    fn a_block_that_does_not_decode_is_magenta() {
        assert_eq!(decode_block(&[0; 16], 4, 4, false)[0], ERROR);
    }
}

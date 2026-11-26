//! AES and SHA (the Arm ARM's AESE ... SHA256SU1, from its AESSubBytes, AESMixColumns, SHAhashSIGMA0 ... functions).

use crate::{Fault, Sem};
use a64::{AesOp, Insn, Sha2Op, Sha3Op};
use mir::{BinOp, Elem, Op, Table, Type, Value};

const B16: Type = Type::Vec(16, Elem::I8);
const S4: Type = Type::Vec(4, Elem::I32);

/// The four words of a register, low first.
type Words = [Value; 4];

impl Sem<'_> {
    pub(crate) fn crypto(&mut self, insn: &Insn) -> Result<(), Fault> {
        match *insn {
            Insn::Aes { op, rd, rn } => {
                let n = self.vread(rn, B16);
                let r = match op {
                    AesOp::Aese | AesOp::Aesd => {
                        let d = self.vread(rd, B16);
                        let state = self.bin(BinOp::Xor, d, n);
                        let inverse = op == AesOp::Aesd;
                        // (ShiftRows: byte (row r, column c) from column c + r; the inverse from c - r)
                        let mask = (0..16u8)
                            .map(|i| {
                                let (c, r) = (i / 4, i % 4);
                                let from = if inverse { (c + 4 - r) % 4 } else { (c + r) % 4 };
                                4 * from + r
                            })
                            .collect();
                        let shifted = self.op(Op::Shuffle(state, state, mask));
                        self.sub_bytes(shifted, if inverse { Table::AesInvSbox } else { Table::AesSbox })
                    }
                    AesOp::Aesmc => self.mix_columns(n, false),
                    AesOp::Aesimc => self.mix_columns(n, true),
                };
                self.vwrite(rd, r);
            }
            Insn::Sha3 { op, rd, rn, rm } => {
                let d = self.words(rd);
                let n = self.words(rn);
                let m = self.words(rm);
                let r = match op {
                    Sha3Op::Sha1c | Sha3Op::Sha1p | Sha3Op::Sha1m => self.sha1_rounds(op, d, n[0], m),
                    Sha3Op::Sha1su0 => {
                        // (Vn's low half : Vd's high half, XOR Vd, XOR Vm)
                        let t = [d[2], d[3], n[0], n[1]];
                        std::array::from_fn(|i| {
                            let x = self.bin(BinOp::Xor, t[i], d[i]);
                            self.bin(BinOp::Xor, x, m[i])
                        })
                    }
                    Sha3Op::Sha256h => self.sha256_hash(d, n, m, true),
                    Sha3Op::Sha256h2 => self.sha256_hash(n, d, m, false),
                    Sha3Op::Sha256su1 => {
                        let t0 = [n[1], n[2], n[3], m[0]];
                        let mut r = d;
                        for e in 0..4 {
                            let src = if e < 2 { m[e + 2] } else { r[e - 2] };
                            let s = self.small_sigma(src, 17, 19, 10);
                            let x = self.bin(BinOp::Add, s, d[e]);
                            r[e] = self.bin(BinOp::Add, x, t0[e]);
                        }
                        r
                    }
                };
                self.write_words(rd, r);
            }
            Insn::Sha2 { op, rd, rn } => {
                let n = self.words(rn);
                match op {
                    Sha2Op::Sha1h => {
                        let r = self.rol(n[0], 30);
                        self.vwrite(rd, r);
                    }
                    Sha2Op::Sha1su1 => {
                        let d = self.words(rd);
                        let zero = self.konst(Type::I32, 0);
                        let shifted = [n[1], n[2], n[3], zero];
                        let t: Words = std::array::from_fn(|i| self.bin(BinOp::Xor, d[i], shifted[i]));
                        let mut w: Words = std::array::from_fn(|i| self.rol(t[i], 1));
                        let extra = self.rol(t[0], 2);
                        w[3] = self.bin(BinOp::Xor, w[3], extra);
                        self.write_words(rd, w);
                    }
                    Sha2Op::Sha256su0 => {
                        let d = self.words(rd);
                        let t = [d[1], d[2], d[3], n[0]];
                        let r = std::array::from_fn(|e| {
                            let s = self.small_sigma(t[e], 7, 18, 3);
                            self.bin(BinOp::Add, s, d[e])
                        });
                        self.write_words(rd, r);
                    }
                }
            }
            _ => unreachable!("not a cryptographic instruction: {insn}"),
        }
        Ok(())
    }

    /// Each byte of `state` through a table.
    fn sub_bytes(&mut self, state: Value, table: Table) -> Value {
        let mut out = state;
        for i in 0..16 {
            let b = self.op(Op::Extract(state, i));
            let s = self.op(Op::Lookup(table, b));
            out = self.op(Op::Insert(out, i, s));
        }
        out
    }

    /// Each byte times 2 in GF(2^8) (the AES polynomial).
    fn xtime(&mut self, x: Value) -> Value {
        let one = self.k_like(x, 1);
        let doubled = self.bin(BinOp::Shl, x, one);
        let seven = self.k_like(x, 7);
        let top = self.bin(BinOp::LShr, x, seven);
        let poly = self.k_like(x, 0x1b);
        let reduce = self.bin(BinOp::Mul, top, poly);
        self.bin(BinOp::Xor, doubled, reduce)
    }

    /// MixColumns, or InvMixColumns (`inverse`), of the 16 bytes (four columns of four).
    fn mix_columns(&mut self, x: Value, inverse: bool) -> Value {
        // (row r of a column from row r + k of the same column)
        let rot = |s: &mut Self, v: Value, k: u8| {
            let mask = (0..16u8).map(|i| (i & !3) | ((i + k) & 3)).collect();
            s.op(Op::Shuffle(v, v, mask))
        };
        let x2 = self.xtime(x);
        // (each output byte: Σ c_k · a_{r+k}, the coefficients 2 3 1 1, or 14 11 13 9)
        let terms: [Value; 4] = if inverse {
            let x4 = self.xtime(x2);
            let x8 = self.xtime(x4);
            let x9 = self.bin(BinOp::Xor, x8, x);
            let x11 = self.bin(BinOp::Xor, x9, x2);
            let x13 = self.bin(BinOp::Xor, x9, x4);
            let x14 = self.bin(BinOp::Xor, x8, x4);
            let x14 = self.bin(BinOp::Xor, x14, x2);
            [x14, x11, x13, x9]
        } else {
            let x3 = self.bin(BinOp::Xor, x2, x);
            [x2, x3, x, x]
        };
        let mut acc = terms[0];
        for (k, &t) in terms.iter().enumerate().skip(1) {
            let r = rot(self, t, k as u8);
            acc = self.bin(BinOp::Xor, acc, r);
        }
        acc
    }

    fn words(&mut self, n: a64::VReg) -> Words {
        let v = self.vread(n, S4);
        std::array::from_fn(|i| self.op(Op::Extract(v, i as u8)))
    }

    fn write_words(&mut self, n: a64::VReg, w: Words) {
        let zero = self.konst(Type::I128, 0);
        let mut v = self.op(Op::Bitcast(S4, zero));
        for (i, &x) in w.iter().enumerate() {
            v = self.op(Op::Insert(v, i as u8, x));
        }
        self.vwrite(n, v);
    }

    fn rol(&mut self, x: Value, n: u32) -> Value {
        self.ror_k(x, 32 - n)
    }

    /// ROR(x, a) ^ ROR(x, b) ^ LSR(x, c).
    fn small_sigma(&mut self, x: Value, a: u32, b: u32, c: u32) -> Value {
        let ra = self.ror_k(x, a);
        let rb = self.ror_k(x, b);
        let s = self.lshr_k(x, c);
        let t = self.bin(BinOp::Xor, ra, rb);
        self.bin(BinOp::Xor, t, s)
    }

    /// SHA1C, SHA1P, SHA1M: four rounds of SHA-1 (choose, parity, majority) on abcd (`x`), e (`y`) and the words.
    fn sha1_rounds(&mut self, op: Sha3Op, mut x: Words, mut y: Value, w: Words) -> Words {
        for &we in &w {
            let t = match op {
                Sha3Op::Sha1c => self.choose(x[1], x[2], x[3]),
                Sha3Op::Sha1p => {
                    let p = self.bin(BinOp::Xor, x[1], x[2]);
                    self.bin(BinOp::Xor, p, x[3])
                }
                _ => self.majority(x[1], x[2], x[3]),
            };
            let a5 = self.rol(x[0], 5);
            let s = self.bin(BinOp::Add, y, a5);
            let s = self.bin(BinOp::Add, s, t);
            y = self.bin(BinOp::Add, s, we);
            x[1] = self.rol(x[1], 30);
            // (<Y, X> = ROL(Y:X, 32))
            let top = x[3];
            x = [y, x[0], x[1], x[2]];
            y = top;
        }
        x
    }

    /// SHAchoose: (y ^ z) & x ^ z.
    fn choose(&mut self, x: Value, y: Value, z: Value) -> Value {
        let t = self.bin(BinOp::Xor, y, z);
        let t = self.bin(BinOp::And, t, x);
        self.bin(BinOp::Xor, t, z)
    }

    /// SHAmajority: (x & y) | ((x | y) & z).
    fn majority(&mut self, x: Value, y: Value, z: Value) -> Value {
        let a = self.bin(BinOp::And, x, y);
        let o = self.bin(BinOp::Or, x, y);
        let b = self.bin(BinOp::And, o, z);
        self.bin(BinOp::Or, a, b)
    }

    /// SHA256hash: four rounds of SHA-256 on abcd (`x`) and efgh (`y`); `part1` gives the new abcd, else efgh.
    fn sha256_hash(&mut self, mut x: Words, mut y: Words, w: Words, part1: bool) -> Words {
        for &we in &w {
            let chs = self.choose(y[0], y[1], y[2]);
            let maj = self.majority(x[0], x[1], x[2]);
            let s1 = self.big_sigma(y[0], 6, 11, 25);
            let t = self.bin(BinOp::Add, y[3], s1);
            let t = self.bin(BinOp::Add, t, chs);
            let t = self.bin(BinOp::Add, t, we);
            x[3] = self.bin(BinOp::Add, t, x[3]);
            let s0 = self.big_sigma(x[0], 2, 13, 22);
            let u = self.bin(BinOp::Add, t, s0);
            y[3] = self.bin(BinOp::Add, u, maj);
            // (<Y, X> = ROL(Y:X, 32))
            let (x3, y3) = (x[3], y[3]);
            x = [y3, x[0], x[1], x[2]];
            y = [x3, y[0], y[1], y[2]];
        }
        if part1 { x } else { y }
    }

    /// ROR(x, a) ^ ROR(x, b) ^ ROR(x, c).
    fn big_sigma(&mut self, x: Value, a: u32, b: u32, c: u32) -> Value {
        let ra = self.ror_k(x, a);
        let rb = self.ror_k(x, b);
        let rc = self.ror_k(x, c);
        let t = self.bin(BinOp::Xor, ra, rb);
        self.bin(BinOp::Xor, t, rc)
    }
}

//! The SIMD and FP registers as operands: reading a register as a value of any type of up to 128 bits, writing one
//! (a write of fewer than 128 bits zeroes the rest of the register, as every Advanced SIMD and FP write does, except
//! the element inserts and the "2" forms' upper halves, which keep it), and the constants of those types.

use crate::Sem;
use a64::{Arr, FpType, Shape, Size, VReg};
use mir::{Elem, Loc, Op, Type, V128, Value};

/// The integer type of an element of `size` (B to D, and Q's `i128`).
pub(crate) fn int_type(size: Size) -> Type {
    Type::int(size.bytes() * 8).expect("an element of 1 to 16 bytes")
}

/// The float type of a format.
pub(crate) fn float_type(fp: FpType) -> Type {
    match fp {
        FpType::H => Type::F16,
        FpType::S => Type::F32,
        FpType::D => Type::F64,
    }
}

/// The vector type of `arr`'s lanes, each of `elem` (1D: the scalar `elem`).
pub(crate) fn vector_of(arr: Arr, elem: Type) -> Type {
    match arr.lanes() {
        1 => elem,
        n => Type::Vec(n as u8, Elem::of(elem).expect("a scalar element")),
    }
}

/// The type of a shape's operand with integer elements: a vector of its lanes, or a scalar.
pub(crate) fn shape_int(shape: Shape) -> Type {
    match shape {
        Shape::Vector(a) => vector_of(a, int_type(a.size)),
        Shape::Scalar(s) => int_type(s),
    }
}

/// The type of a shape's operand with float elements (H, S or D).
pub(crate) fn shape_float(shape: Shape) -> Type {
    shape_int(shape).with_elem(Type::float(shape.size().bytes() * 8).expect("a float element"))
}

impl Sem<'_> {
    /// A constant of `ty` (integer or float, scalar or vector) with every element's bits `bits`.
    pub(crate) fn splat_k(&mut self, ty: Type, bits: u128) -> Value {
        let elem = ty.elem();
        let int = self.konst(elem.as_int(), bits);
        let scalar = if elem.is_float() { self.op(Op::Bitcast(elem, int)) } else { int };
        if ty.is_vector() { self.op(Op::Splat(ty, scalar)) } else { scalar }
    }

    /// A constant of `v`'s type, each element `bits`.
    pub(crate) fn k_like(&mut self, v: Value, bits: u128) -> Value {
        let ty = self.ty(v);
        self.splat_k(ty, bits)
    }

    /// `v` as a type of the same width (nothing when it is of it; a bitcast back is the value bitcast).
    pub(crate) fn cast_to(&mut self, v: Value, ty: Type) -> Value {
        if self.ty(v) == ty {
            return v;
        }
        let source = match self.f.inst(v).op {
            Op::Bitcast(_, x) => x,
            _ => v,
        };
        if self.ty(source) == ty { source } else { self.op(Op::Bitcast(ty, source)) }
    }

    /// `v`'s bits as integers of its shape (`f32` to `i32`, `<4 x float>` to `<4 x i32>`).
    pub(crate) fn as_int(&mut self, v: Value) -> Value {
        let ty = self.ty(v).as_int();
        self.cast_to(v, ty)
    }

    /// `x` in every element of `ty` (`x` itself when `ty` is a scalar).
    pub(crate) fn splat_to(&mut self, ty: Type, x: Value) -> Value {
        if ty.is_vector() { self.op(Op::Splat(ty, x)) } else { x }
    }

    /// A register's 128 bits.
    pub(crate) fn vget(&mut self, n: VReg) -> Value {
        self.get(Loc::V(n.0))
    }

    /// A register's low bits as a value of `ty` (8 to 128 bits).
    pub(crate) fn vread(&mut self, n: VReg, ty: Type) -> Value {
        let v = self.vget(n);
        match ty.bits() {
            128 => self.cast_to(v, ty),
            bits => {
                let lo = self.op(Op::Extract(v, 0));
                let int = Type::int(bits).expect("8 to 64 bits");
                let lo = self.zext(lo, int);
                self.cast_to(lo, ty)
            }
        }
    }

    /// A register's upper 64 bits as a value of `ty` (of 64 bits).
    pub(crate) fn vread_hi(&mut self, n: VReg, ty: Type) -> Value {
        let v = self.vget(n);
        let hi = self.op(Op::Extract(v, 1));
        self.cast_to(hi, ty)
    }

    /// The operand of `shape` in register `n`; `upper` reads a vector's elements from the register's upper half (the
    /// "2" forms' narrow sources: `shape` is then of 64 bits).
    pub(crate) fn vread_part(&mut self, n: VReg, ty: Type, upper: bool) -> Value {
        if upper { self.vread_hi(n, ty) } else { self.vread(n, ty) }
    }

    /// `v` (8 to 128 bits) as a register's whole value: its bits low, zeros above.
    pub(crate) fn whole_register(&mut self, v: Value) -> Value {
        let ty = self.ty(v);
        if ty.bits() == 128 {
            return self.cast_to(v, V128);
        }
        let int = Type::int(ty.bits()).expect("8 to 64 bits");
        let bits = self.cast_to(v, int);
        let lo = self.zext(bits, Type::I64);
        let zero = self.konst(Type::I128, 0);
        let zero = self.op(Op::Bitcast(V128, zero));
        self.op(Op::Insert(zero, 0, lo))
    }

    /// Writes register `n`: `v`'s bits (8 to 128 of them), the rest of the register zeroed.
    pub(crate) fn vwrite(&mut self, n: VReg, v: Value) {
        let whole = self.whole_register(v);
        self.set(Loc::V(n.0), whole);
    }

    /// Writes the upper 64 bits of register `n` with `v` (of 64 bits), keeping the lower ones.
    pub(crate) fn vwrite_hi(&mut self, n: VReg, v: Value) {
        let old = self.vget(n);
        let bits = self.cast_to(v, Type::I64);
        let new = self.op(Op::Insert(old, 1, bits));
        self.set(Loc::V(n.0), new);
    }

    /// Writes a narrow result: to the low half (zeroing the upper) or, for the "2" forms (`upper`), to the upper half.
    pub(crate) fn vwrite_part(&mut self, n: VReg, v: Value, upper: bool) {
        if upper { self.vwrite_hi(n, v) } else { self.vwrite(n, v) }
    }

    /// Writes element `lane` of register `n` (of `v`'s type), keeping the others.
    pub(crate) fn vwrite_lane(&mut self, n: VReg, lane: u8, v: Value) {
        let elem = self.ty(v).as_int();
        let bits = self.cast_to(v, elem);
        let lanes = (128 / elem.bits()) as u8;
        let ty = Type::Vec(lanes, Elem::of(elem).expect("an element"));
        let old = self.vread(n, ty);
        let new = self.op(Op::Insert(old, lane, bits));
        self.vwrite(n, new);
    }

    /// Element `lane` of register `n`, of `elem`'s type.
    pub(crate) fn vlane(&mut self, n: VReg, elem: Type, lane: u8) -> Value {
        let int = elem.as_int();
        let lanes = (128 / int.bits()) as u8;
        let v = self.vread(n, Type::Vec(lanes, Elem::of(int).expect("an element")));
        let x = self.op(Op::Extract(v, lane));
        self.cast_to(x, elem)
    }
}

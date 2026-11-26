//! The guest as the game's Rust sees it: the lifted code's context ([`Ctx`]), guest memory through typed, checked
//! addresses ([`GuestPtr`]), and the host's services the game's code calls ([`host`]). This is the one crate of the
//! game's with `unsafe` code: the game's own crates (game/crates/oot) take a [`GuestCtx`] and stay safe.
//!
//! A read or a write of guest memory goes through the host's functions (host/game_guest.c), which are the lift's own
//! readers and writers (`recomp_load*`, `recomp_store*`): a store marks its pages, as the lifted code's do, so the
//! renderer's captures and the write watch see it. Before it, the address is checked: not null, aligned for its type,
//! and inside the guest window. A check that fails gives `None` (the C tested a pointer for null alone before it followed it).
//!
//! A word other threads change at the same time (the SDK's sync objects' state) goes through [`atomic`] instead:
//! [`GuestPtr::atomic`] requires the caller to guarantee the word’s lifetime and compatible access. Each write marks the page.
// (deny, not forbid, in the workspace: this crate is where the game's Rust meets the host's C and guest memory)
#![allow(unsafe_code)]

pub mod atomic;
pub mod host;

pub use atomic::{GuestAtomicU32, GuestAtomicU64};

use std::marker::PhantomData;

/// The lifted code's context: the guest's registers, as the lift lays them out (builder/rt/a64.h `GuestContext`, the
/// IR's offsets in builder/crates/llvm). Field for field; the asserts below hold the layout to the C's.
#[repr(C)]
#[derive(Debug)]
pub struct Ctx {
    /// X0 to X30; `x[31]` is SP.
    pub x: [u64; 32],
    /// Written only where a thread stops.
    pub pc: u64,
    /// NZCV, a byte each.
    pub nzcv: [u8; 4],
    /// The thread stopped.
    pub halted: i32,
    /// Q0 to Q31, two 64-bit halves each (the low half first).
    pub vreg: [[u64; 2]; 32],
    /// The guest's own thread pointer.
    pub tpidr_el0: u64,
    /// Horizon's thread-local region.
    pub tpidrro_el0: u64,
    /// The floating-point control register.
    pub fpcr: u64,
    /// The floating-point status register.
    pub fpsr: u64,
    /// The exclusive monitor's address.
    pub excl_addr: u64,
    /// The exclusive monitor's low value.
    pub excl_lo: u64,
    /// The exclusive monitor's high value.
    pub excl_hi: u64,
    /// The exclusive monitor's size.
    pub excl_size: u32,
    /// Whether the exclusive monitor holds.
    pub excl_valid: i32,
    /// The OS layer's thread object.
    pub thread: *mut core::ffi::c_void,
    /// An interrupt request.
    pub poll: u32,
    /// The entry block of a split function's next piece.
    pub piece: u32,
}

const _: () = {
    assert!(std::mem::offset_of!(Ctx, vreg) == 272);
    assert!(std::mem::offset_of!(Ctx, fpcr) == 800);
    assert!(std::mem::offset_of!(Ctx, poll) == 856);
    assert!(std::mem::offset_of!(Ctx, piece) == 860);
    assert!(std::mem::size_of::<Ctx>() == 864);
};

/// A value guest memory holds at an address: one of the widths the host reads and writes.
pub trait Scalar: Copy {
    /// Its size in bytes (1, 2, 4 or 8); its alignment is the same.
    const SIZE: u32;
    /// From the bits the host read (zero-extended).
    fn from_bits(bits: u64) -> Self;
    /// To the bits the host writes.
    fn to_bits(self) -> u64;
}

macro_rules! scalar {
    ($t:ty, $size:expr, |$b:ident| $from:expr, |$v:ident| $to:expr) => {
        impl Scalar for $t {
            const SIZE: u32 = $size;
            fn from_bits($b: u64) -> Self {
                $from
            }
            fn to_bits(self) -> u64 {
                let $v = self;
                $to
            }
        }
    };
}
scalar!(u8, 1, |b| b as u8, |v| u64::from(v));
scalar!(u16, 2, |b| b as u16, |v| u64::from(v));
scalar!(u32, 4, |b| b as u32, |v| u64::from(v));
scalar!(u64, 8, |b| b, |v| v);
scalar!(f32, 4, |b| f32::from_bits(b as u32), |v| u64::from(v.to_bits()));
scalar!(f64, 8, |b| f64::from_bits(b), |v| v.to_bits());

/// A guest address of a `T`: a value read from guest memory or a register, not yet followed. Making one checks nothing;
/// [`GuestCtx::read`] and [`GuestCtx::write`] check it. `T` is a [`Scalar`] to read, or a type of the game's (an
/// uninhabited marker, as `enum Camera {}`) whose fields are reached with [`GuestPtr::field`].
pub struct GuestPtr<T> {
    addr: u64,
    of: PhantomData<fn() -> T>,
}

impl<T> Clone for GuestPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for GuestPtr<T> {}

impl<T> std::fmt::Debug for GuestPtr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GuestPtr({:#x})", self.addr)
    }
}

impl<T> PartialEq for GuestPtr<T> {
    fn eq(&self, other: &Self) -> bool {
        self.addr == other.addr
    }
}
impl<T> Eq for GuestPtr<T> {}

impl<T> GuestPtr<T> {
    /// The address `addr`.
    pub const fn new(addr: u64) -> GuestPtr<T> {
        GuestPtr { addr, of: PhantomData }
    }

    /// Its address.
    pub const fn addr(self) -> u64 {
        self.addr
    }

    /// Whether it is 0.
    pub const fn is_null(self) -> bool {
        self.addr == 0
    }

    /// The field of a `U` at `offset` bytes in (wrapping, as the guest's own address arithmetic: the read checks it).
    pub const fn field<U>(self, offset: u64) -> GuestPtr<U> {
        GuestPtr::new(self.addr.wrapping_add(offset))
    }

    /// The same address as a `U`.
    pub const fn cast<U>(self) -> GuestPtr<U> {
        GuestPtr::new(self.addr)
    }
}

/// The address of `size` bytes at `addr` when it may be followed: not null, aligned to `size`, all inside the window.
fn checked(addr: u64, size: u32) -> Option<u64> {
    let size = u64::from(size);
    let end = host::guest_end();
    (addr != 0 && addr.is_multiple_of(size) && addr < end && size <= end - addr).then_some(addr)
}

/// The guest context a hook site's callback runs with: its registers, and guest memory through it. Made from the
/// context the lifted code passed (`Option<&mut Ctx>` in the callback's `extern "C"` signature: the lift passes its
/// context, never null, and it is the calling thread's own for the call).
pub struct GuestCtx<'a> {
    ctx: &'a mut Ctx,
}

impl<'a> GuestCtx<'a> {
    /// The context the lifted code passed.
    pub fn new(ctx: &'a mut Ctx) -> GuestCtx<'a> {
        GuestCtx { ctx }
    }

    /// The registers, as the lifted code left them.
    pub fn regs(&self) -> &Ctx {
        self.ctx
    }

    /// X`n` (X31: SP).
    pub fn x(&self, n: usize) -> u64 {
        self.ctx.x[n]
    }

    /// S`n`: the low 32 bits of V`n`, as a float.
    pub fn vreg_f32(&self, n: usize) -> f32 {
        f32::from_bits(self.ctx.vreg[n][0] as u32)
    }

    /// S`n` written as a scalar float write does: V`n`'s low 32 bits the value's, the rest of the register cleared
    /// (a64.h `a64_vset64` of the value's bits).
    pub fn set_vreg_f32(&mut self, n: usize, v: f32) {
        self.ctx.vreg[n] = [u64::from(v.to_bits()), 0];
    }

    /// D`n`: the low 64 bits of V`n`, as a double.
    pub fn vreg_f64(&self, n: usize) -> f64 {
        f64::from_bits(self.ctx.vreg[n][0])
    }

    /// D`n` written as a scalar double write does: the rest of the register cleared.
    pub fn set_vreg_f64(&mut self, n: usize, v: f64) {
        self.ctx.vreg[n] = [v.to_bits(), 0];
    }

    /// The `T` at `p`, or `None` when `p` is null, not aligned for a `T` or not all inside the guest window.
    pub fn read<T: Scalar>(&mut self, p: GuestPtr<T>) -> Option<T> {
        let addr = checked(p.addr, T::SIZE)?;
        // SAFETY: the address is inside the guest window and aligned (checked); hos_guest_load reads it as the lift's
        // recomp_load* does, with this thread's own context (self.ctx, borrowed for the call).
        let bits = unsafe { host::hos_guest_load(self.ctx, addr, T::SIZE) };
        Some(T::from_bits(bits))
    }

    /// The pointer at `p` (a guest address read from guest memory), or `None` as [`GuestCtx::read`].
    pub fn read_ptr<U>(&mut self, p: GuestPtr<GuestPtr<U>>) -> Option<GuestPtr<U>> {
        self.read(p.cast::<u64>()).map(GuestPtr::new)
    }

    /// `v` written at `p`, its pages marked; false (nothing written) when `p` is null, not aligned or not all inside
    /// the guest window.
    pub fn write<T: Scalar>(&mut self, p: GuestPtr<T>, v: T) -> bool {
        let Some(addr) = checked(p.addr, T::SIZE) else { return false };
        // SAFETY: as in read; hos_guest_store writes as recomp_store* does and marks the pages it wrote.
        unsafe { host::hos_guest_store(self.ctx, addr, T::SIZE, v.to_bits()) };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_is_followed_only_when_it_may_be() {
        let end = host::guest_end();
        assert_eq!(checked(0, 8), None, "null");
        assert_eq!(checked(0x1004, 8), None, "not aligned");
        assert_eq!(checked(0x1008, 8), Some(0x1008));
        assert_eq!(checked(end - 8, 8), Some(end - 8), "the window's last word");
        assert_eq!(checked(end - 4, 8), None, "past the window's end");
        assert_eq!(checked(u64::MAX - 7, 8), None, "past the window, wrapping");
    }

    /// A context of all zeros (plain data and a null thread pointer: all zero bits is a valid one).
    fn zeroed() -> Box<Ctx> {
        // SAFETY: every field of Ctx is an integer, an array of them or a raw pointer, for which zero bits are valid.
        Box::new(unsafe { std::mem::MaybeUninit::<Ctx>::zeroed().assume_init() })
    }

    #[test]
    fn a_float_write_clears_the_rest_of_the_register() {
        let mut ctx = zeroed();
        ctx.vreg[0] = [u64::MAX, u64::MAX];
        let mut g = GuestCtx::new(&mut ctx);
        g.set_vreg_f32(0, 1.5);
        assert_eq!(g.regs().vreg[0], [u64::from(1.5f32.to_bits()), 0]);
        assert_eq!(g.vreg_f32(0), 1.5);
    }

    #[test]
    fn reads_and_writes_go_through_the_host_with_their_checks() {
        let mut ctx = zeroed();
        let mut g = GuestCtx::new(&mut ctx);
        let p = GuestPtr::<u32>::new(0x2000);
        assert!(g.write(p, 0xdead_beef));
        assert_eq!(g.read(p), Some(0xdead_beef));
        assert_eq!(g.read(GuestPtr::<u16>::new(0x2002)), Some(0xdead), "the high half, little-endian");
        assert!(!g.write(GuestPtr::<u32>::new(0x2001), 1), "not aligned: not written");
        assert_eq!(g.read(GuestPtr::<u64>::new(0)), None, "null");
        assert!(g.write(GuestPtr::<u64>::new(0x3000), 0x4000));
        let to = g.read_ptr(GuestPtr::<GuestPtr<f32>>::new(0x3000)).unwrap();
        assert_eq!(to.addr(), 0x4000);
        assert_eq!(to.field::<u8>(3).addr(), 0x4003);
    }
}

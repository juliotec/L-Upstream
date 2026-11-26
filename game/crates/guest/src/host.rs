//! The host's services the game's Rust calls (host/game_host.h): guest memory (host/game_guest.c) and the options
//! (host/rx_backend.c). Declared once, here; the safe wrappers are what the game's crates call.
//!
//! A test of this crate has no host: its functions are stand-ins over a map of guest memory of its own.

#[cfg(not(test))]
use crate::Ctx;

#[cfg(not(test))]
unsafe extern "C" {
    /// The guest window's end (host/game_guest.c).
    safe fn hos_guest_end() -> u64;
    /// `size` bytes at `a`, as the lift's `recomp_load*`.
    ///
    /// # Safety
    ///
    /// `a` is checked first (aligned to `size`, inside the window) and `c` is the calling thread's own context.
    pub fn hos_guest_load(c: *mut Ctx, a: u64, size: u32) -> u64;
    /// `v` written at `a` as the lift's `recomp_store*`, its pages marked.
    ///
    /// # Safety
    ///
    /// As [`hos_guest_load`].
    pub fn hos_guest_store(c: *mut Ctx, a: u64, size: u32, v: u64);
    /// An option's value as the Insert menu has it, else `otherwise` (host/rx_backend.c).
    safe fn rx_option(id: i32, otherwise: i32) -> i32;
    /// The host address of the aligned 4- or 8-byte word at `a` in the guest window, or null (host/game_guest.c).
    safe fn hos_guest_word(a: u64, size: u32) -> *mut core::ffi::c_void;
    /// The pages of `size` bytes at `a` marked as written, as the lift's stores mark them.
    safe fn hos_guest_mark(a: u64, size: u32);
    /// Sleeps while the 32-bit word at `a` holds `expected`, at most `ms` (`u32::MAX`: no limit); 1 woken or changed.
    safe fn hos_guest_wait32(a: u64, expected: u32, ms: u32) -> i32;
    /// Wakes one sleeper on the 32-bit word at `a`, or all of them.
    safe fn hos_guest_wake32(a: u64, all: i32);
}

/// The guest window's end: an address below it is in the window.
pub fn guest_end() -> u64 {
    hos_guest_end()
}

/// The option `id`'s value (game/options.h's ids), else `otherwise`.
pub fn option(id: i32, otherwise: i32) -> i32 {
    rx_option(id, otherwise)
}

/// The host address of the aligned `size`-byte word at `a` in the guest window, or null.
pub(crate) fn word(a: u64, size: u32) -> *mut core::ffi::c_void {
    hos_guest_word(a, size)
}

/// The pages of `size` bytes at `a` marked as written.
pub(crate) fn mark(a: u64, size: u32) {
    hos_guest_mark(a, size);
}

/// Sleeps while the 32-bit word at `a` holds `expected`, at most `ms` milliseconds (`u32::MAX`: no limit).
pub(crate) fn wait32(a: u64, expected: u32, ms: u32) -> bool {
    hos_guest_wait32(a, expected, ms) != 0
}

/// Wakes one sleeper on the 32-bit word at `a`, or all of them.
pub(crate) fn wake32(a: u64, all: bool) {
    hos_guest_wake32(a, i32::from(all));
}

#[cfg(test)]
pub use fake::*;

#[cfg(test)]
mod fake {
    //! The host for this crate's tests: a guest window of 1 MB and a map of the bytes written into it.
    use crate::Ctx;
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static BYTES: RefCell<HashMap<u64, u8>> = RefCell::new(HashMap::new());
    }

    /// The stand-in window's end: 1 MB.
    pub fn hos_guest_end() -> u64 {
        1 << 20
    }

    /// The stand-in's read: the bytes written before, else 0.
    ///
    /// # Safety
    ///
    /// None needed by the stand-in; unsafe as the host's own is.
    pub unsafe fn hos_guest_load(_c: *mut Ctx, a: u64, size: u32) -> u64 {
        BYTES.with(|b| {
            let b = b.borrow();
            (0..u64::from(size)).fold(0, |v, i| v | u64::from(*b.get(&(a + i)).unwrap_or(&0)) << (8 * i))
        })
    }

    /// The stand-in's write, into its map.
    ///
    /// # Safety
    ///
    /// None needed by the stand-in; unsafe as the host's own is.
    pub unsafe fn hos_guest_store(_c: *mut Ctx, a: u64, size: u32, v: u64) {
        BYTES.with(|b| {
            let mut b = b.borrow_mut();
            for i in 0..u64::from(size) {
                b.insert(a + i, (v >> (8 * i)) as u8);
            }
        });
    }

    /// The stand-in's options: none set, every one its default.
    pub fn rx_option(_id: i32, otherwise: i32) -> i32 {
        otherwise
    }

    /// The stand-in's words: 64 KB of its own from address 0, aligned, never freed.
    pub fn hos_guest_word(a: u64, size: u32) -> *mut core::ffi::c_void {
        use std::sync::OnceLock;
        static WORDS: OnceLock<usize> = OnceLock::new();
        let base = *WORDS.get_or_init(|| Box::leak(vec![0u64; 8192].into_boxed_slice()).as_mut_ptr() as usize);
        if (size != 4 && size != 8) || !a.is_multiple_of(u64::from(size)) || a + u64::from(size) > 65536 {
            return std::ptr::null_mut();
        }
        (base + a as usize) as *mut core::ffi::c_void
    }

    thread_local! {
        /// The marks the stand-in was asked for, (address, size).
        pub static MARKS: RefCell<Vec<(u64, u32)>> = const { RefCell::new(Vec::new()) };
    }

    /// The stand-in's mark: noted.
    pub fn hos_guest_mark(a: u64, size: u32) {
        MARKS.with(|m| m.borrow_mut().push((a, size)));
    }

    /// The stand-in's wait: none (as a spurious wake).
    pub fn hos_guest_wait32(_a: u64, _expected: u32, _ms: u32) -> i32 {
        1
    }

    /// The stand-in's wake: nothing sleeps.
    pub fn hos_guest_wake32(_a: u64, _all: i32) {}
}

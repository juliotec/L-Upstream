//! Guest words as atomics: the SDK's own sync words (a mutex's owner, an event's counter, a queue's count, a multi-wait's
//! lists), which the host's sync objects keep their state in. Plain game data goes through [`crate::GuestCtx::read`] and
//! [`crate::GuestCtx::write`]; a word other threads change at the same time goes through these.
//!
//! The word is checked as a read is (aligned, all inside the guest window, the address taken whole: base plus the full
//! 64-bit address, as the lift's `A64_PTR`), then worked on in place with the host's atomics, sequentially consistent.
//! Every write marks the word's pages as the lift's stores do, so a sync word on a page the GPU maps is captured with
//! its value. A 32-bit word can be slept on until it changes ([`GuestAtomicU32::wait`]), as the SDK's waits do.

use crate::{GuestPtr, host};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst};

macro_rules! guest_atomic {
    ($name:ident, $atomic:ty, $int:ty, $size:expr) => {
        /// A guest word, worked on atomically in place; made from a [`GuestPtr`] with [`GuestPtr::atomic`].
        #[derive(Clone, Copy)]
        pub struct $name {
            addr: u64,
            word: *mut $int,
        }

        // SAFETY: construction requires live memory with synchronized or same-width atomic access, including across threads.
        unsafe impl Send for $name {}
        unsafe impl Sync for $name {}

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}({:#x})", stringify!($name), self.addr)
            }
        }

        impl GuestPtr<$int> {
            /// The word as an atomic, or `None` when it is null, not aligned or not all inside the guest window.
            ///
            /// ```compile_fail,E0133
            /// use guest::GuestPtr;
            ///
            #[doc = concat!("if false { let _ = GuestPtr::<", stringify!($int), ">::new(0x100).atomic(); }")]
            /// ```
            ///
            /// # Safety
            ///
            /// The word must remain valid for reads and writes for every use of this handle or any copy. All overlapping
            /// accesses, including those from the lift and the host, must use same-width atomics or be synchronized with
            /// the handle's accesses. The window and alignment checks do not establish either guarantee.
            pub unsafe fn atomic(self) -> Option<$name> {
                if self.is_null() {
                    return None;
                }
                let p = host::word(self.addr(), $size) as *mut $int;
                if p.is_null() {
                    return None;
                }
                Some($name { addr: self.addr(), word: p })
            }
        }

        impl $name {
            fn word(&self) -> &$atomic {
                // SAFETY: the caller who constructed the handle guarantees validity and compatible access.
                unsafe { <$atomic>::from_ptr(self.word) }
            }

            /// The guest address.
            pub fn addr(&self) -> u64 {
                self.addr
            }

            /// Its value.
            pub fn load(&self) -> $int {
                self.word().load(SeqCst)
            }

            /// `v` stored, the page marked.
            pub fn store(&self, v: $int) {
                self.word().store(v, SeqCst);
                host::mark(self.addr, $size);
            }

            /// `v` stored, the value before returned; the page marked.
            pub fn swap(&self, v: $int) -> $int {
                let old = self.word().swap(v, SeqCst);
                host::mark(self.addr, $size);
                old
            }

            /// `new` stored if the word holds `current`: `Ok(current)`, the page marked; else `Err(the value it holds)`.
            pub fn compare_exchange(&self, current: $int, new: $int) -> Result<$int, $int> {
                let r = self.word().compare_exchange(current, new, SeqCst, SeqCst);
                if r.is_ok() {
                    host::mark(self.addr, $size);
                }
                r
            }

            /// `v` added (wrapping), the value before returned; the page marked.
            pub fn fetch_add(&self, v: $int) -> $int {
                let old = self.word().fetch_add(v, SeqCst);
                host::mark(self.addr, $size);
                old
            }

            /// `v` subtracted (wrapping), the value before returned; the page marked.
            pub fn fetch_sub(&self, v: $int) -> $int {
                let old = self.word().fetch_sub(v, SeqCst);
                host::mark(self.addr, $size);
                old
            }

            /// `v` or'd in, the value before returned; the page marked.
            pub fn fetch_or(&self, v: $int) -> $int {
                let old = self.word().fetch_or(v, SeqCst);
                host::mark(self.addr, $size);
                old
            }

            /// `v` and'ed in, the value before returned; the page marked.
            pub fn fetch_and(&self, v: $int) -> $int {
                let old = self.word().fetch_and(v, SeqCst);
                host::mark(self.addr, $size);
                old
            }
        }
    };
}

guest_atomic!(GuestAtomicU32, AtomicU32, u32, 4);
guest_atomic!(GuestAtomicU64, AtomicU64, u64, 8);

impl GuestAtomicU32 {
    /// Sleeps while the word holds `expected`, at most `ms` milliseconds (`None`: no limit): true when woken or the word
    /// changed, false when the time ran out. A wake may be spurious: read the word again.
    pub fn wait(&self, expected: u32, ms: Option<u32>) -> bool {
        host::wait32(self.addr, expected, ms.unwrap_or(u32::MAX))
    }

    /// Wakes one thread sleeping on the word.
    pub fn wake_one(&self) {
        host::wake32(self.addr, false);
    }

    /// Wakes every thread sleeping on the word.
    pub fn wake_all(&self) {
        host::wake32(self.addr, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks() -> Vec<(u64, u32)> {
        host::MARKS.with(|m| std::mem::take(&mut *m.borrow_mut()))
    }

    #[test]
    fn a_word_is_followed_only_when_it_may_be() {
        // SAFETY: the stub owns the backing memory and the test uses disjoint addresses.
        unsafe {
            assert!(GuestPtr::<u32>::new(0).atomic().is_none(), "null");
            assert!(GuestPtr::<u32>::new(0x102).atomic().is_none(), "not aligned");
            assert!(GuestPtr::<u64>::new(0x104).atomic().is_none(), "not aligned for 8");
            assert!(GuestPtr::<u32>::new(0x100).atomic().is_some());
        }
    }

    #[test]
    fn every_write_marks_and_a_failed_exchange_does_not() {
        // SAFETY: only this test uses these disjoint words in the stub's static backing memory.
        let w = unsafe { GuestPtr::<u32>::new(0x200).atomic().unwrap() };
        marks();
        w.store(5);
        assert_eq!(w.compare_exchange(5, 7), Ok(5));
        assert_eq!(w.compare_exchange(5, 9), Err(7), "holds 7");
        assert_eq!(w.fetch_add(3), 7);
        assert_eq!(w.swap(1), 10);
        assert_eq!(w.load(), 1);
        assert_eq!(marks(), vec![(0x200, 4); 4], "store, exchange, add, swap: a mark each; the failed exchange none");
        let d = unsafe { GuestPtr::<u64>::new(0x208).atomic().unwrap() };
        assert_eq!(d.fetch_or(0x10), 0);
        assert_eq!(d.fetch_and(0x30), 0x10);
        assert_eq!(d.fetch_sub(1), 0x10);
        assert_eq!(marks(), vec![(0x208, 8); 3]);
    }
}

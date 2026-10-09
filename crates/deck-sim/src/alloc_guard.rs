//! Counts heap allocations made inside `forbid(..)`, in any build profile.
//! The simulator wraps every engine callback in it and fails if the count
//! moves: the audio thread must never allocate. `count(..)` measures
//! allocations without calling them violations (the UI bench).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::LocalKey;

pub struct Guarded;

static VIOLATIONS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static FORBIDDEN: Cell<bool> = const { Cell::new(false) };
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTED: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: every method forwards unchanged to `System`, which upholds the
// `GlobalAlloc` contract; `note` only touches const-initialised thread-locals
// (no destructors, no lazy setup) and an atomic counter, and never allocates.
unsafe impl GlobalAlloc for Guarded {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's guarantees for `layout` are passed straight on.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        // SAFETY: `ptr` came from `alloc`/`realloc` above, i.e. from `System`,
        // with this `layout`, as the caller guarantees.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: as for `dealloc`; `new_size` is checked by the caller.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn note() {
    if FORBIDDEN.get() {
        VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    }
    if COUNTING.get() {
        COUNTED.set(COUNTED.get() + 1);
    }
}

/// Sets a thread-local flag and puts its previous value back when dropped,
/// so nested calls and panics leave the flag as they found it.
struct Raised {
    flag: &'static LocalKey<Cell<bool>>,
    was: bool,
}

impl Raised {
    fn new(flag: &'static LocalKey<Cell<bool>>) -> Self {
        Self { flag, was: flag.replace(true) }
    }
}

impl Drop for Raised {
    fn drop(&mut self) {
        self.flag.set(self.was);
    }
}

/// Run `f`, counting any allocation or free it makes on this thread as a
/// violation.
pub fn forbid<R>(f: impl FnOnce() -> R) -> R {
    let _raised = Raised::new(&FORBIDDEN);
    f()
}

/// Run `f` and return how many allocations, reallocations and frees it made
/// on this thread. Not violations: for code that should be allocation-free
/// in steady state but is measured, not policed.
pub fn count<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = COUNTED.get();
    let r = {
        let _raised = Raised::new(&COUNTING);
        f()
    };
    (r, COUNTED.get().wrapping_sub(before))
}

pub fn violations() -> u64 {
    VIOLATIONS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbid_restores_the_flag_when_nested() {
        forbid(|| {
            forbid(|| {});
            assert!(FORBIDDEN.get());
        });
        assert!(!FORBIDDEN.get());
    }

    #[test]
    fn forbid_restores_the_flag_after_a_panic() {
        let caught = std::panic::catch_unwind(|| forbid(|| std::panic::panic_any("boom")));
        assert!(caught.is_err());
        assert!(!FORBIDDEN.get());
    }

    #[test]
    fn count_sees_allocations_on_this_thread() {
        let (v, n) = count(|| vec![1u8; 64]);
        assert_eq!(v.len(), 64);
        assert!(n >= 1);
        let ((), n) = count(|| {});
        assert_eq!(n, 0);
    }
}

//! Tokenizer-only Rust heap ceiling, distinct from an RSS claim. Track allocations
//! from startup; ordinary decoder mode irreversibly disables accounting before IPC.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Allocator {
    used: AtomicUsize,
    // Zero is a terminal disabled state. No method can enable it again.
    limit: AtomicUsize,
}
impl Allocator {
    pub const fn new() -> Self {
        Self {
            used: AtomicUsize::new(0),
            limit: AtomicUsize::new(usize::MAX),
        }
    }
    pub fn disable(&self) {
        self.limit.store(0, Ordering::SeqCst);
    }
    pub fn limit(&self, bytes: usize) -> Result<(), ()> {
        self.limit
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                (current != 0 && bytes != 0).then_some(bytes)
            })
            .map_err(|_| ())?;
        if self.used.load(Ordering::SeqCst) > bytes {
            Err(())
        } else {
            Ok(())
        }
    }
    fn disabled(&self) -> bool {
        self.limit.load(Ordering::Relaxed) == 0
    }
    fn reserve(&self, bytes: usize) -> bool {
        self.used
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                used.checked_add(bytes)
                    .filter(|next| *next <= self.limit.load(Ordering::SeqCst))
            })
            .is_ok()
    }
}
// SAFETY: System owns all pointers; accounting never changes requested layout,
// alignment, pointer lifetime, or the allocator's null-on-failure contract.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if self.disabled() {
            return unsafe { System.alloc(layout) };
        }
        if !self.reserve(layout.size()) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            self.used.fetch_sub(layout.size(), Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        if !self.disabled() {
            self.used.fetch_sub(layout.size(), Ordering::SeqCst);
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        if self.disabled() {
            return unsafe { System.realloc(ptr, old, new_size) };
        }
        // Reserve full replacement to bound the transient old+new heap, even if
        // System could sometimes resize in place. A failure preserves old ptr.
        if !self.reserve(new_size) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.realloc(ptr, old, new_size) };
        self.used.fetch_sub(
            if p.is_null() { new_size } else { old.size() },
            Ordering::SeqCst,
        );
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_bypass_is_permanent_and_stops_accounting_startup_allocations() {
        let allocator = Allocator::new();
        allocator.limit(64).unwrap();
        let small = Layout::from_size_align(16, 8).unwrap();
        let large = Layout::from_size_align(128, 8).unwrap();
        unsafe {
            let original = allocator.alloc(small);
            assert!(!original.is_null());
            assert_eq!(allocator.used.load(Ordering::SeqCst), 16);
            allocator.disable();
            assert!(allocator.limit(64).is_err());
            allocator.dealloc(original, small);
            let over_old_cap = allocator.alloc(large);
            assert!(!over_old_cap.is_null());
            let replaced = allocator.realloc(over_old_cap, large, 256);
            assert!(!replaced.is_null());
            allocator.dealloc(replaced, Layout::from_size_align(256, 8).unwrap());
            assert_eq!(allocator.used.load(Ordering::SeqCst), 16);
            assert!(allocator.disabled());
        }
    }

    #[test]
    fn tokenizer_heap_tracks_startup_and_bounds_transient_reallocations() {
        let allocator = Allocator::new();
        let initial = Layout::from_size_align(16, 8).unwrap();
        unsafe {
            let original = allocator.alloc(initial);
            assert!(!original.is_null());
            allocator.limit(40).unwrap();
            let refused = allocator.realloc(original, initial, 32);
            assert!(refused.is_null());
            assert_eq!(allocator.used.load(Ordering::SeqCst), 16);
            let replacement = allocator.realloc(original, initial, 24);
            assert!(!replacement.is_null());
            assert_eq!(allocator.used.load(Ordering::SeqCst), 24);
            allocator.dealloc(replacement, Layout::from_size_align(24, 8).unwrap());
            assert_eq!(allocator.used.load(Ordering::SeqCst), 0);
        }
    }
}

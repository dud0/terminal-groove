pub mod audio;
mod dsp;
mod engine;
mod generator;
pub mod model;
pub mod persistence;
mod reducer;
pub mod tui;

#[cfg(test)]
mod test_allocator {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
        static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }

    pub struct CountingAllocator;

    // The counter is deliberately test-only. Production audio code keeps the
    // platform allocator unchanged and does not pay for these atomics.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            ALLOCATIONS.with(|count| count.set(count.get() + 1));
            // SAFETY: forwarding the caller's valid layout to the system allocator.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            // SAFETY: forwarding the pointer/layout pair supplied by the allocator contract.
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    pub fn reset() {
        ALLOCATIONS.with(|count| count.set(0));
        DEALLOCATIONS.with(|count| count.set(0));
    }

    pub fn counts() -> (usize, usize) {
        (ALLOCATIONS.with(Cell::get), DEALLOCATIONS.with(Cell::get))
    }
}

#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: test_allocator::CountingAllocator = test_allocator::CountingAllocator;

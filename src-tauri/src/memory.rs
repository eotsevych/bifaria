//! Returns freed audio buffers to the OS on Linux.
//!
//! Each dictation allocates multi-megabyte buffers (the recording, its resampled and leveled
//! copies, the model's scratch) and frees them seconds later. glibc serves big allocations with
//! mmap and unmaps them on free, but after the first such free it raises that threshold (up to
//! 32 MB), so later buffers come from malloc's arenas. Arena memory freed by the app stays cached
//! and is never returned, so Bifaria's memory grows with every dictation.
//!
//! Adapted from Handy (https://github.com/cjpais/Handy, MIT License, Copyright (c) 2025 CJ Pais),
//! which measured about 15 MB kept per 2-minute dictation, and about 0.5 MB with this fix.
//! macOS, Windows and musl don't have this problem; there both functions do nothing.

/// Pins glibc's mmap threshold so big buffers keep going back to the OS when freed. Call before
/// anything large is allocated.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn init_allocator() {
    // SAFETY: mallopt takes no pointers; it only changes malloc's parameters.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn init_allocator() {}

/// Hands free arena pages back to the OS. Takes about a millisecond; called once per dictation.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn trim_freed_memory() {
    // SAFETY: malloc_trim takes no pointers and is thread-safe.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn trim_freed_memory() {}

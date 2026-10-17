//! Main stack high-water mark: the free part of the stack is painted once at boot and the
//! deepest overwritten word is found by scanning from the bottom (on every heap log).

const PAINT: u32 = 0xA5A5_A5A5;
/// Skips the stack-guard word esp-hal keeps near the bottom (`stack-guard-offset` is 60).
const BOTTOM_SKIP: usize = 64;
/// Leaves the frames that are live while painting alone.
const LIVE_MARGIN: usize = 512;

unsafe extern "C" {
    static _stack_end: u32; // lowest address of the main stack
    static _stack_start: u32; // highest address (the stack grows down from here)
}

fn bounds() -> (usize, usize) {
    // only the linker symbols' addresses are used
    (
        core::ptr::addr_of!(_stack_end) as usize + BOTTOM_SKIP,
        core::ptr::addr_of!(_stack_start) as usize,
    )
}

/// Paints the unused main stack. Call once, early in `main`.
pub fn paint() {
    let (bottom, _) = bounds();
    let sp: usize;
    // SAFETY: reads the stack pointer only
    unsafe { core::arch::asm!("mv {}, sp", out(reg) sp) };
    let mut addr = bottom;
    while addr + 4 <= sp.saturating_sub(LIVE_MARGIN) {
        // SAFETY: [bottom, sp - margin) is inside the main stack and not in use yet
        unsafe { core::ptr::write_volatile(addr as *mut u32, PAINT) };
        addr += 4;
    }
}

/// `(peak used bytes, stack size)` since boot.
pub fn high_water() -> (usize, usize) {
    let (bottom, top) = bounds();
    let mut addr = bottom;
    // SAFETY: reads within the main stack
    while addr < top && unsafe { core::ptr::read_volatile(addr as *const u32) } == PAINT {
        addr += 4;
    }
    (top - addr, top - bottom + BOTTOM_SKIP)
}

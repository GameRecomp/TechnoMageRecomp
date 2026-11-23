// SPDX-License-Identifier: MIT
//! Guest memory: the guest's 32-bit address space as one host region.
//!
//! On 64-bit hosts the full 4 GiB (plus a guard for an 8-byte access at the top) are reserved,
//! so every guest address is valid host memory and the accessors need no bounds check; the
//! host commits pages only when they are touched. On 32-bit hosts (the browser) the region is
//! smaller and every access is checked.

use std::sync::OnceLock;

/// Bytes past 4 GiB, so that an access of up to 8 bytes at any 32-bit address stays inside.
const GUARD: u64 = 16;
const FULL: u64 = (1 << 32) + GUARD;

/// The memory a process's guest code runs on. It lives for the rest of the program.
pub struct GuestMemory {
    base: *mut u8,
    len: u64,
}

// SAFETY: the region is shared by the guest threads, which run one at a time.
unsafe impl Send for GuestMemory {}
// SAFETY: as above; concurrent host access (the audio thread) uses its own copies.
unsafe impl Sync for GuestMemory {}

static MEMORY: OnceLock<GuestMemory> = OnceLock::new();

impl GuestMemory {
    /// The process's guest memory, reserved on first use. `len` is used on 32-bit hosts only.
    pub fn get(len: u64) -> &'static GuestMemory {
        MEMORY.get_or_init(|| GuestMemory::reserve(len))
    }

    fn reserve(len: u64) -> GuestMemory {
        let len = if cfg!(target_pointer_width = "64") {
            FULL
        } else {
            len
        };
        // Alignment 16, not a page: then the allocator serves a zeroed block this large with
        // fresh anonymous pages (calloc → mmap) that the host commits on first touch, instead of
        // clearing 4 GiB. (Windows commits the whole block up front: a reserve/commit scheme
        // there is part of M3r.)
        let layout =
            std::alloc::Layout::from_size_align(len as usize, 16).expect("guest memory layout");
        // SAFETY: the layout has a non-zero size.
        let base = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(
            !base.is_null(),
            "cannot reserve {len} bytes of guest memory"
        );
        GuestMemory { base, len }
    }

    pub fn mem(&self) -> Mem {
        let len = if self.len == FULL { u64::MAX } else { self.len };
        Mem {
            base: self.base,
            len,
        }
    }
}

/// A handle to guest memory, cheap to copy: what lifted code reads and writes through.
#[derive(Debug, Clone, Copy)]
pub struct Mem {
    base: *mut u8,
    /// `u64::MAX`: the full address space plus guard, no checks needed.
    len: u64,
}

// SAFETY: guest memory is never unmapped, and the guest threads that share it run one at a
// time (`threads`), as on the single-CPU machines the games were made for.
unsafe impl Send for Mem {}
// SAFETY: as above; host helpers that split work over threads (the software rasterizer) write
// disjoint bytes.
unsafe impl Sync for Mem {}

macro_rules! access {
    ($ld:ident, $st:ident, $t:ty, $n:expr) => {
        #[inline(always)]
        pub fn $ld(self, a: u32) -> $t {
            let p = self.at(a, $n);
            // SAFETY: `at` returned a pointer with `$n` valid bytes behind it.
            unsafe { (p as *const $t).read_unaligned() }
        }
        #[inline(always)]
        pub fn $st(self, a: u32, v: $t) {
            let p = self.at(a, $n);
            // SAFETY: as above; guest memory is writable everywhere.
            unsafe { (p as *mut $t).write_unaligned(v) }
        }
    };
}

impl Mem {
    /// # Safety
    /// `base`/`len` must come from [`Mem::raw`] of a live `Mem`.
    #[inline(always)]
    pub(crate) unsafe fn from_raw(base: *mut u8, len: u64) -> Mem {
        assert!(!base.is_null(), "no guest memory");
        Mem { base, len }
    }

    pub(crate) fn raw(self) -> (*mut u8, u64) {
        (self.base, self.len)
    }

    #[inline(always)]
    fn at(self, a: u32, n: u64) -> *mut u8 {
        if self.len != u64::MAX && u64::from(a) + n > self.len {
            panic!("guest memory access at {a:#010x} beyond {:#x}", self.len);
        }
        // SAFETY: with the full reservation every 32-bit address plus `n <= 8` bytes is inside;
        // otherwise the check above holds.
        unsafe { self.base.add(a as usize) }
    }

    access!(ld8, st8, u8, 1);
    access!(ld16, st16, u16, 2);
    access!(ld32, st32, u32, 4);
    access!(ld64, st64, u64, 8);

    /// Copy guest bytes out.
    pub fn read(self, a: u32, out: &mut [u8]) {
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.ld8(a.wrapping_add(i as u32));
        }
    }

    /// Copy bytes into guest memory.
    pub fn write(self, a: u32, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.st8(a.wrapping_add(i as u32), b);
        }
    }

    /// A NUL-terminated guest string (bytes, without the NUL), at most `max` long.
    pub fn cstr(self, a: u32, max: usize) -> Vec<u8> {
        let mut v = Vec::new();
        for i in 0..max as u32 {
            let b = self.ld8(a.wrapping_add(i));
            if b == 0 {
                break;
            }
            v.push(b);
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_and_stores_are_little_endian_and_unaligned() {
        let m = GuestMemory::get(1 << 20).mem();
        m.st32(0x1001, 0x1122_3344);
        assert_eq!(m.ld8(0x1001), 0x44);
        assert_eq!(m.ld16(0x1002), 0x2233);
        assert_eq!(m.ld32(0x1001), 0x1122_3344);
        m.write(0x2000, b"abc\0def");
        assert_eq!(m.cstr(0x2000, 100), b"abc");
    }
}

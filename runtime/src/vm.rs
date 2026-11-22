// SPDX-License-Identifier: MIT
//! Guest address space management: page regions (`VirtualAlloc`) and heaps (`HeapAlloc`,
//! `malloc` underneath the CRT). Bookkeeping lives on the host, the blocks in guest memory.

use std::collections::BTreeMap;
use std::sync::Mutex;

pub const PAGE: u32 = 0x1000;
pub const GRANULE: u32 = 0x10000;
/// Dynamic allocations go here: above the executable's image, below the TIB.
pub const DYN_LO: u32 = 0x0200_0000;
pub const DYN_HI: u32 = 0x7f00_0000;

struct Space {
    /// Reserved regions: base → size (granule aligned).
    regions: BTreeMap<u32, u32>,
    /// Heap blocks: address → size; free blocks: address → size.
    used: BTreeMap<u32, u32>,
    free: BTreeMap<u32, u32>,
    lo: u32,
}

static SPACE: Mutex<Space> = Mutex::new(Space {
    regions: BTreeMap::new(),
    used: BTreeMap::new(),
    free: BTreeMap::new(),
    lo: DYN_LO,
});

/// Move the start of dynamic allocations up (above a large image).
pub fn set_floor(addr: u32) {
    let mut s = SPACE.lock().expect("vm");
    let a = addr.div_ceil(GRANULE) * GRANULE;
    if a > s.lo {
        s.lo = a;
    }
}

fn find_gap(s: &Space, size: u32) -> Option<u32> {
    let mut at = s.lo;
    for (&b, &n) in &s.regions {
        if b >= at && b - at >= size {
            return Some(at);
        }
        at = at.max(b + n);
    }
    (DYN_HI.checked_sub(at)? >= size).then_some(at)
}

/// Reserve (and commit: guest memory is always backed) `size` bytes; `want` asks for an
/// address. Returns 0 when it is taken or nothing fits.
pub fn region_alloc(want: u32, size: u32) -> u32 {
    let size = size.div_ceil(GRANULE).max(1) * GRANULE;
    let mut s = SPACE.lock().expect("vm");
    let base = if want != 0 {
        let b = want / GRANULE * GRANULE;
        // Inside an existing region: committing pages of it.
        if let Some((&rb, &rn)) = s.regions.range(..=b).next_back() {
            if b < rb + rn {
                return want / PAGE * PAGE;
            }
        }
        if s.regions.range(b..b + size).next().is_some() {
            return 0;
        }
        b
    } else {
        match find_gap(&s, size) {
            Some(b) => b,
            None => return 0,
        }
    };
    s.regions.insert(base, size);
    base
}

/// Release the region starting at `base`. The bytes stay (guest memory is never unmapped),
/// but a later allocation may reuse them; a fresh one is zeroed by the caller if it must be.
pub fn region_free(base: u32) -> bool {
    SPACE.lock().expect("vm").regions.remove(&base).is_some()
}

pub fn region_size(addr: u32) -> Option<(u32, u32)> {
    let s = SPACE.lock().expect("vm");
    let (&b, &n) = s.regions.range(..=addr).next_back()?;
    (addr < b + n).then_some((b, n))
}

const ALIGN: u32 = 16;
const CHUNK: u32 = 0x0010_0000;

/// A heap block of `size` bytes (zeroed when `zero`), 16-byte aligned; 0 when out of memory.
pub fn heap_alloc(m: crate::mem::Mem, size: u32, zero: bool) -> u32 {
    let want = size.max(1).div_ceil(ALIGN) * ALIGN;
    let mut s = SPACE.lock().expect("vm");
    let fit = s
        .free
        .iter()
        .find(|(_, &n)| n >= want)
        .map(|(&a, &n)| (a, n));
    let (a, n) = match fit {
        Some(f) => f,
        None => {
            drop(s);
            let grow = want.max(CHUNK);
            let b = region_alloc(0, grow);
            if b == 0 {
                return 0;
            }
            s = SPACE.lock().expect("vm");
            let size = grow.div_ceil(GRANULE) * GRANULE;
            s.free.insert(b, size);
            (b, size)
        }
    };
    s.free.remove(&a);
    if n > want {
        s.free.insert(a + want, n - want);
    }
    s.used.insert(a, want);
    drop(s);
    if zero {
        for i in 0..want {
            m.st8(a + i, 0);
        }
    }
    a
}

/// Free a heap block; false if `a` is not one.
pub fn heap_free(a: u32) -> bool {
    let mut s = SPACE.lock().expect("vm");
    let Some(n) = s.used.remove(&a) else {
        return false;
    };
    let (mut start, mut len) = (a, n);
    // Merge with the free neighbours.
    if let Some((&pa, &pn)) = s.free.range(..a).next_back() {
        if pa + pn == a {
            s.free.remove(&pa);
            start = pa;
            len += pn;
        }
    }
    if let Some(&nn) = s.free.get(&(a + n)) {
        s.free.remove(&(a + n));
        len += nn;
    }
    s.free.insert(start, len);
    true
}

/// The size of a live heap block (its rounded size).
pub fn heap_size(a: u32) -> Option<u32> {
    SPACE.lock().expect("vm").used.get(&a).copied()
}

/// Resize a block: in place when it fits, else a new block with the contents copied.
pub fn heap_realloc(m: crate::mem::Mem, a: u32, size: u32, zero: bool) -> u32 {
    let Some(old) = heap_size(a) else {
        return 0;
    };
    let want = size.max(1).div_ceil(ALIGN) * ALIGN;
    if want <= old {
        return a;
    }
    let b = heap_alloc(m, size, zero);
    if b == 0 {
        return 0;
    }
    for i in 0..old {
        m.st8(b + i, m.ld8(a + i));
    }
    heap_free(a);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::GuestMemory;

    #[test]
    fn heap_blocks_are_distinct_and_reused() {
        let m = GuestMemory::get(1 << 30).mem();
        let a = heap_alloc(m, 100, true);
        let b = heap_alloc(m, 100, false);
        assert!(a != 0 && b != 0 && a != b);
        assert_eq!(heap_size(a), Some(112));
        assert!(heap_free(a));
        let c = heap_alloc(m, 50, false);
        assert_eq!(c, a);
        let d = heap_realloc(m, b, 5000, false);
        assert!(d != 0 && heap_size(d).unwrap() >= 5000);
    }
}

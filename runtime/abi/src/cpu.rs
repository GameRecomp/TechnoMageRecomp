// SPDX-License-Identifier: MIT
//! The guest CPU state, with the sister projects' `rc_cpu` layout: the Cranelift blobs
//! (`rc-emit-native`) and the WebAssembly modules (`rc-emit-wasm`) address its fields by offset.

use crate::mem::Mem;

/// Register numbers in encoding order.
pub const EAX: usize = 0;
pub const ECX: usize = 1;
pub const EDX: usize = 2;
pub const EBX: usize = 3;
pub const ESP: usize = 4;
pub const EBP: usize = 5;
pub const ESI: usize = 6;
pub const EDI: usize = 7;

/// Offsets native code compiles against (`rc-emit-native`'s `layout`).
pub const MEM_OFFSET: usize = std::mem::offset_of!(Cpu, mem);
pub const HELPERS_OFFSET: usize = std::mem::offset_of!(Cpu, helpers);

/// The state of one guest thread.
#[repr(C)]
#[derive(Debug)]
pub struct Cpu {
    /// General-purpose registers in encoding order.
    pub r: [u32; 8],
    /// Arithmetic flags, one byte each (0 or 1).
    pub cf: u8,
    pub pf: u8,
    pub af: u8,
    pub zf: u8,
    pub sf: u8,
    pub of: u8,
    /// Direction flag for string instructions.
    pub df: u8,
    _pad: u8,
    /// x87 registers as host doubles (fast mode), the top of stack, control, status and tag.
    pub st: [f64; 8],
    pub top: u8,
    pub fpu_control: u16,
    pub fpu_status: u16,
    pub fpu_tag: u16,
    /// Base of the segment `fs` addresses: the thread information block.
    pub fs_base: u32,
    /// The guest address being executed, for diagnostics only.
    pub eip: u32,
    /// Base of guest memory, for native and WebAssembly code; set only by the runtime.
    mem: *mut u8,
    /// The runtime functions native code calls (`rc-emit-native`'s helper table).
    #[doc(hidden)]
    pub helpers: *const *const (),
    /// MMX registers (kept apart from the x87 registers).
    pub mm: [u64; 8],
    /// Exceptions: while `unwind` is set, lifted functions return after each call until the
    /// frame with `unwind_ebp` that holds `unwind_eip` takes it over.
    pub unwind: u32,
    pub unwind_eip: u32,
    pub unwind_ebp: u32,
    pub eh_depth: u32,
    pub eh_esp: u32,
    /// The continuation `unwind_claim` is entering (an `__except` block lets it in).
    pub resume_eip: u32,
    /// Bytes of guest memory behind `mem` (u64::MAX: the full 4 GiB are reserved).
    mem_len: u64,
}

// SAFETY: a Cpu belongs to one guest thread; guest threads run one at a time (the baton in
// `threads`), and the memory behind `mem` outlives every Cpu (it is never unmapped).
unsafe impl Send for Cpu {}

impl Cpu {
    /// A fresh CPU on guest memory `m`: registers zero, x87 after `finit`.
    pub fn new(m: Mem) -> Self {
        let (mem, mem_len) = m.raw();
        Cpu {
            r: [0; 8],
            cf: 0,
            pf: 0,
            af: 0,
            zf: 0,
            sf: 0,
            of: 0,
            df: 0,
            _pad: 0,
            st: [0.0; 8],
            top: 0,
            fpu_control: 0x037f,
            fpu_status: 0,
            fpu_tag: 0xffff,
            fs_base: 0,
            eip: 0,
            mem,
            helpers: std::ptr::null(),
            mm: [0; 8],
            unwind: 0,
            unwind_eip: 0,
            unwind_ebp: 0,
            eh_depth: 0,
            eh_esp: 0,
            resume_eip: 0,
            mem_len,
        }
    }

    /// Guest memory.
    #[inline(always)]
    pub fn mem(&self) -> Mem {
        // SAFETY: `mem`/`mem_len` were taken from a valid `Mem` in `new` and are never changed.
        unsafe { Mem::from_raw(self.mem, self.mem_len) }
    }

    /// EFLAGS as the guest sees it (`pushfd`).
    pub fn eflags(&self) -> u32 {
        u32::from(self.cf)
            | 0x2
            | (u32::from(self.pf) << 2)
            | (u32::from(self.af) << 4)
            | (u32::from(self.zf) << 6)
            | (u32::from(self.sf) << 7)
            | (u32::from(self.df) << 10)
            | (u32::from(self.of) << 11)
    }

    pub fn set_eflags(&mut self, f: u32) {
        self.cf = (f & 1) as u8;
        self.pf = ((f >> 2) & 1) as u8;
        self.af = ((f >> 4) & 1) as u8;
        self.zf = ((f >> 6) & 1) as u8;
        self.sf = ((f >> 7) & 1) as u8;
        self.df = ((f >> 10) & 1) as u8;
        self.of = ((f >> 11) & 1) as u8;
    }

    pub fn push32(&mut self, v: u32) {
        self.r[ESP] = self.r[ESP].wrapping_sub(4);
        self.mem().st32(self.r[ESP], v);
    }

    pub fn pop32(&mut self) -> u32 {
        let v = self.mem().ld32(self.r[ESP]);
        self.r[ESP] = self.r[ESP].wrapping_add(4);
        v
    }

    /// The `n`th 32-bit stack argument of an stdcall/cdecl function just called (0 = first).
    pub fn arg(&self, n: u32) -> u32 {
        self.mem().ld32(self.r[ESP].wrapping_add(4 + 4 * n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// The offsets the native and WebAssembly back ends compile against (`rc_cpu` in C).
    #[test]
    fn layout_matches_rc_cpu() {
        assert_eq!(offset_of!(Cpu, cf), 32);
        assert_eq!(offset_of!(Cpu, df), 38);
        assert_eq!(offset_of!(Cpu, st), 40);
        assert_eq!(offset_of!(Cpu, top), 104);
        assert_eq!(offset_of!(Cpu, fpu_control), 106);
        assert_eq!(offset_of!(Cpu, fpu_status), 108);
        assert_eq!(offset_of!(Cpu, fpu_tag), 110);
        assert_eq!(offset_of!(Cpu, fs_base), 112);
        assert_eq!(offset_of!(Cpu, eip), 116);
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(offset_of!(Cpu, mem), 120);
            assert_eq!(offset_of!(Cpu, helpers), 128);
            assert_eq!(offset_of!(Cpu, mm), 136);
            assert_eq!(offset_of!(Cpu, unwind), 200);
            assert_eq!(offset_of!(Cpu, resume_eip), 220);
        }
    }
}

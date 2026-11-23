// SPDX-License-Identifier: MIT
//! Everything generated code (`rc-emit-rust`) names: `use rc_abi::lifted::*;`.

pub use crate::cpu::Cpu;

/// A lifted (or hand-written) guest function.
pub type GuestFn = fn(&mut Cpu);

// The runtime's entry points. `rc-runtime` defines each with `#[no_mangle]` and exactly this
// signature (its `dispatch` module); a program that links lifted code links the runtime too.
// SAFETY: the definitions match these declarations (checked by the runtime's tests, which call
// them through this block).
unsafe extern "Rust" {
    /// An indirect call: the return address is already pushed.
    #[link_name = "rc_rt_dispatch"]
    pub safe fn dispatch(c: &mut Cpu, target: u32);
    /// An indirect jump.
    #[link_name = "rc_rt_dispatch_jump"]
    pub safe fn dispatch_jump(c: &mut Cpu, target: u32);
    /// After a call returned while unwinding: this frame may take the continuation over.
    #[link_name = "rc_rt_unwind_claim"]
    pub safe fn unwind_claim(c: &mut Cpu, ebp: u32, lo: u32, hi: u32, at: &[u32]);
    /// An `__except` block entered during an exception: unwind to its frame first.
    #[link_name = "rc_rt_unwind_begin"]
    pub safe fn unwind_begin(c: &mut Cpu, eip: u32);
    /// A lifted instruction the runtime cannot execute.
    #[link_name = "rc_rt_trap"]
    pub safe fn trap(c: &mut Cpu, addr: u32, why: &str) -> !;
    /// A patch point.
    #[link_name = "rc_rt_point"]
    pub safe fn point(c: &mut Cpu, addr: u32);
    #[link_name = "rc_rt_cpuid"]
    pub safe fn cpuid(eax: &mut u32, ebx: &mut u32, ecx: &mut u32, edx: &mut u32);
    #[link_name = "rc_rt_rdtsc"]
    pub safe fn rdtsc(eax: &mut u32, edx: &mut u32);
}
pub use crate::mem::Mem;
pub use crate::mmx::*;
pub use crate::x87::{
    f32_bits, f32_of, f64_of, f80, farith, fbinary, fcom, fconst, ffree, finit, fist, fldenv, fpop,
    fpush, frstor, fsave, fstenv, fstsw, funary, fxch, set_st, st, st_f80,
};

/// One lifted binary: its identity and its sorted function table.
pub struct GeneratedBinary {
    pub name: &'static str,
    pub variant: &'static str,
    /// SHA-256 of the file as shipped (packed executables: the packed file).
    pub sha256: &'static str,
    pub base: u32,
    pub entry: u32,
    pub table: &'static [(u32, GuestFn)],
}

/// Every binary lifted for a game.
pub struct GeneratedSet {
    pub game: &'static str,
    pub binaries: &'static [&'static GeneratedBinary],
}

macro_rules! shifts {
    ($shl:ident, $shr:ident, $sar:ident, $t:ty, $s:ty, $bits:expr) => {
        #[inline(always)]
        pub fn $shl(a: $t, n: u32) -> $t {
            if n >= $bits {
                0
            } else {
                a << n
            }
        }
        #[inline(always)]
        pub fn $shr(a: $t, n: u32) -> $t {
            if n >= $bits {
                0
            } else {
                a >> n
            }
        }
        #[inline(always)]
        pub fn $sar(a: $t, n: u32) -> $t {
            ((a as $s) >> n.min($bits - 1)) as $t
        }
    };
}

shifts!(shl8, shr8, sar8, u8, i8, 8);
shifts!(shl16, shr16, sar16, u16, i16, 16);
shifts!(shl32, shr32, sar32, u32, i32, 32);
shifts!(shl64, shr64, sar64, u64, i64, 64);

/// 1 when the byte has an even number of set bits (PF).
#[inline(always)]
pub fn parity8(v: u8) -> u8 {
    u8::from(v.count_ones().is_multiple_of(2))
}

#[inline(always)]
pub fn ctz32(v: u32) -> u32 {
    v.trailing_zeros()
}
#[inline(always)]
pub fn clz32(v: u32) -> u32 {
    v.leading_zeros()
}
#[inline(always)]
pub fn ctz16(v: u16) -> u16 {
    v.trailing_zeros() as u16
}
#[inline(always)]
pub fn clz16(v: u16) -> u16 {
    v.leading_zeros() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_as_the_c_helpers() {
        assert_eq!(shl32(1, 32), 0);
        assert_eq!(shr8(0x80, 7), 1);
        assert_eq!(sar16(0x8000, 20), 0xffff);
        assert_eq!(parity8(0b11), 1);
        assert_eq!(parity8(0b1), 0);
    }
}

// SPDX-License-Identifier: MIT
//! The x87 in the fast mode: host doubles, as the sister projects' `rc/x87.h` and `x87.c`.

use crate::cpu::Cpu;
use crate::mem::Mem;

pub const FSW_C0: u16 = 0x0100;
pub const FSW_C1: u16 = 0x0200;
pub const FSW_C2: u16 = 0x0400;
pub const FSW_C3: u16 = 0x4000;
const FSW_CC: u16 = FSW_C0 | FSW_C1 | FSW_C2 | FSW_C3;

#[inline(always)]
fn slot(c: &Cpu, i: u8) -> usize {
    usize::from(c.top.wrapping_add(i) & 7)
}

/// `st(i)`.
#[inline(always)]
pub fn st(c: &Cpu, i: u8) -> f64 {
    c.st[slot(c, i)]
}

#[inline(always)]
pub fn set_st(c: &mut Cpu, i: u8, v: f64) {
    let s = slot(c, i);
    c.st[s] = v;
}

#[inline(always)]
pub fn fpush(c: &mut Cpu, v: f64) {
    c.top = c.top.wrapping_sub(1) & 7;
    c.st[usize::from(c.top)] = v;
    c.fpu_tag &= !(3u16 << (2 * u16::from(c.top)));
}

#[inline(always)]
pub fn fpop(c: &mut Cpu) {
    c.fpu_tag |= 3u16 << (2 * u16::from(c.top));
    c.top = c.top.wrapping_add(1) & 7;
}

/// A result rounded to single precision when the control word asks for it.
#[inline(always)]
fn fprec(c: &Cpu, v: f64) -> f64 {
    if (c.fpu_control >> 8) & 3 == 0 {
        f64::from(v as f32)
    } else {
        v
    }
}

fn fset_cc(c: &mut Cpu, cc: u16) {
    c.fpu_status = (c.fpu_status & !FSW_CC) | cc;
}

#[inline(always)]
pub fn fstsw(c: &Cpu) -> u16 {
    (c.fpu_status & !0x3800) | (u16::from(c.top) << 11)
}

#[inline(always)]
pub fn f32_of(b: u32) -> f64 {
    f64::from(f32::from_bits(b))
}

#[inline(always)]
pub fn f64_of(b: u64) -> f64 {
    f64::from_bits(b)
}

#[inline(always)]
pub fn f32_bits(v: f64) -> u32 {
    (v as f32).to_bits()
}

/// An 80-bit extended value from guest memory.
pub fn f80(m: Mem, a: u32) -> f64 {
    let sig = m.ld64(a);
    let se = m.ld16(a.wrapping_add(8));
    let e = i32::from(se & 0x7fff);
    let v = if e == 0x7fff {
        if sig << 1 != 0 {
            f64::NAN
        } else {
            f64::INFINITY
        }
    } else if e == 0 && sig == 0 {
        0.0
    } else {
        ldexp(sig as f64, e - 16383 - 63)
    };
    if se >> 15 != 0 {
        -v
    } else {
        v
    }
}

pub fn st_f80(m: Mem, a: u32, v: f64) {
    let mut sig: u64 = 0;
    let mut se: u16 = if v.is_sign_negative() { 0x8000 } else { 0 };
    if v.is_nan() {
        se |= 0x7fff;
        sig = 0xc000_0000_0000_0000;
    } else if v.is_infinite() {
        se |= 0x7fff;
        sig = 0x8000_0000_0000_0000;
    } else if v != 0.0 {
        let (f, e) = frexp(v.abs());
        sig = ldexp(f, 64) as u64;
        se |= (e - 1 + 16383) as u16;
    }
    m.st64(a, sig);
    m.st16(a.wrapping_add(8), se);
}

fn ldexp(x: f64, e: i32) -> f64 {
    // Two steps keep intermediate powers of two finite for |e| up to about 2000.
    let half = e / 2;
    x * 2f64.powi(half) * 2f64.powi(e - half)
}

/// `x = f * 2^e` with `f` in [0.5, 1).
fn frexp(x: f64) -> (f64, i32) {
    if x == 0.0 || !x.is_finite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    if exp == 0 {
        // Subnormal: scale up first.
        let (f, e) = frexp(x * 2f64.powi(64));
        return (f, e - 64);
    }
    let e = exp - 1022;
    let f = f64::from_bits((bits & !(0x7ff << 52)) | (1022 << 52));
    (f, e)
}

fn fround(c: &Cpu, v: f64) -> f64 {
    match (c.fpu_control >> 10) & 3 {
        0 => v.round_ties_even(),
        1 => v.floor(),
        2 => v.ceil(),
        _ => v.trunc(),
    }
}

/// `st(0)` rounded by the control word as an integer of `bits` bits (the integer indefinite
/// when it does not fit).
pub fn fist(c: &Cpu, bits: u32) -> u64 {
    let r = fround(c, st(c, 0));
    let lim = ldexp(1.0, bits as i32 - 1);
    if !(r >= -lim && r < lim) {
        return 1u64 << (bits - 1);
    }
    r as i64 as u64
}

pub fn farith(c: &mut Cpu, op: u32, dst: u8, src: f64) {
    let a = st(c, dst);
    let r = match op {
        0 => a + src,
        1 => a * src,
        2 => a - src,
        3 => src - a,
        4 => a / src,
        _ => src / a,
    };
    let r = fprec(c, r);
    set_st(c, dst, r);
}

pub fn fcom(c: &mut Cpu, b: f64) {
    let a = st(c, 0);
    let cc = if a.is_nan() || b.is_nan() {
        FSW_C3 | FSW_C2 | FSW_C0
    } else if a < b {
        FSW_C0
    } else if a == b {
        FSW_C3
    } else {
        0
    };
    fset_cc(c, cc);
}

pub fn fxch(c: &mut Cpu, i: u8) {
    let t = st(c, 0);
    let v = st(c, i);
    set_st(c, 0, v);
    set_st(c, i, t);
}

pub fn ffree(c: &mut Cpu, i: u8) {
    let p = slot(c, i) as u16;
    c.fpu_tag |= 3 << (2 * p);
}

pub fn funary(c: &mut Cpu, op: u32) {
    let x = st(c, 0);
    match op {
        0 => set_st(c, 0, -x),
        1 => set_st(c, 0, x.abs()),
        2 => set_st(c, 0, fprec(c, x.sqrt())),
        3 | 4 => {
            let v = if op == 3 { x.sin() } else { x.cos() };
            set_st(c, 0, fprec(c, v));
            let keep = c.fpu_status & (FSW_C0 | FSW_C3);
            fset_cc(c, keep);
        }
        5 => set_st(c, 0, fround(c, x)),
        6 => set_st(c, 0, fprec(c, (x * std::f64::consts::LN_2).exp_m1())),
        7 => fcom(c, 0.0),
        _ => {
            let p = c.top;
            let mut cc = if x.is_sign_negative() { FSW_C1 } else { 0 };
            if (c.fpu_tag >> (2 * u16::from(p))) & 3 == 3 {
                cc |= FSW_C3 | FSW_C0;
            } else if x.is_nan() {
                cc |= FSW_C0;
            } else if x.is_infinite() {
                cc |= FSW_C2 | FSW_C0;
            } else if x == 0.0 {
                cc |= FSW_C3;
            } else if x.is_subnormal() {
                cc |= FSW_C3 | FSW_C2;
            } else {
                cc |= FSW_C2;
            }
            fset_cc(c, cc);
        }
    }
}

pub fn fbinary(c: &mut Cpu, op: u32) {
    let (a, b) = (st(c, 0), st(c, 1));
    match op {
        0 => {
            set_st(c, 1, fprec(c, b.atan2(a)));
            fpop(c);
        }
        1 => {
            set_st(c, 1, fprec(c, b * a.log2()));
            fpop(c);
        }
        2 => set_st(c, 0, fprec(c, ldexp(a, b.trunc() as i32))),
        3 | 4 => {
            let q = if op == 3 {
                (a / b).trunc()
            } else {
                (a / b).round_ties_even()
            };
            let r = if op == 3 { a % b } else { a - q * b };
            let qi = q.abs() as u64;
            let mut cc = 0;
            if qi & 1 != 0 {
                cc |= FSW_C1;
            }
            if qi & 2 != 0 {
                cc |= FSW_C3;
            }
            if qi & 4 != 0 {
                cc |= FSW_C0;
            }
            set_st(c, 0, r);
            fset_cc(c, cc);
        }
        _ => {
            set_st(c, 0, fprec(c, a.tan()));
            fpush(c, 1.0);
            let keep = c.fpu_status & (FSW_C0 | FSW_C3);
            fset_cc(c, keep);
        }
    }
}

pub fn fconst(c: &mut Cpu, k: u32) {
    const V: [f64; 7] = [
        1.0,
        0.0,
        std::f64::consts::PI,
        std::f64::consts::LOG2_E,
        std::f64::consts::LOG2_10,
        std::f64::consts::LOG10_2,
        std::f64::consts::LN_2,
    ];
    fpush(c, V[k as usize]);
}

pub fn finit(c: &mut Cpu) {
    c.fpu_control = 0x037f;
    c.fpu_status = 0;
    c.fpu_tag = 0xffff;
    c.top = 0;
}

pub fn fstenv(c: &mut Cpu, m: Mem, a: u32) {
    let w = [
        u32::from(c.fpu_control),
        u32::from(fstsw(c)),
        u32::from(c.fpu_tag),
        0,
        0,
        0,
        0,
    ];
    for (i, v) in w.iter().enumerate() {
        m.st32(a.wrapping_add(4 * i as u32), *v);
    }
    c.fpu_control |= 0x3f;
}

pub fn fldenv(c: &mut Cpu, m: Mem, a: u32) {
    let w0 = m.ld32(a);
    let w1 = m.ld32(a.wrapping_add(4));
    let w2 = m.ld32(a.wrapping_add(8));
    c.fpu_control = w0 as u16;
    c.fpu_status = (w1 & !0x3800) as u16;
    c.top = ((w1 >> 11) & 7) as u8;
    c.fpu_tag = w2 as u16;
}

pub fn fsave(c: &mut Cpu, m: Mem, a: u32) {
    let control = c.fpu_control;
    fstenv(c, m, a);
    m.st32(a, u32::from(control));
    for i in 0..8u8 {
        st_f80(m, a.wrapping_add(28 + 10 * u32::from(i)), st(c, i));
    }
    finit(c);
}

pub fn frstor(c: &mut Cpu, m: Mem, a: u32) {
    fldenv(c, m, a);
    for i in 0..8u8 {
        let v = f80(m, a.wrapping_add(28 + 10 * u32::from(i)));
        set_st(c, i, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::GuestMemory;

    #[test]
    fn extended_values_round_trip() {
        let m = GuestMemory::get(1 << 20).mem();
        for v in [1.0, -2.5, 1e300, 3.0e-310, 0.1, f64::INFINITY] {
            st_f80(m, 0x3000, v);
            assert_eq!(f80(m, 0x3000), v);
        }
    }

    #[test]
    fn stack_and_arithmetic() {
        let m = GuestMemory::get(1 << 20).mem();
        let mut c = Cpu::new(m);
        fpush(&mut c, 2.0);
        fpush(&mut c, 3.0);
        let v = st(&c, 1);
        farith(&mut c, 1, 0, v); // st0 = 3 * 2
        assert_eq!(st(&c, 0), 6.0);
        c.fpu_control = 0x037f; // to nearest
        assert_eq!(fist(&c, 32), 6);
        fpop(&mut c);
        assert_eq!(st(&c, 0), 2.0);
        assert_eq!(fstsw(&c) >> 11 & 7, 7);
    }
}

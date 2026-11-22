// SPDX-License-Identifier: MIT
//! MMX lane operations, as the sister projects' `rc/mmx.h`.

fn lane(v: u64, bits: u32, i: u32) -> u64 {
    (v >> (bits * i)) & ((1u64 << bits) - 1)
}

fn put(v: u64, bits: u32, i: u32, x: u64) -> u64 {
    v | ((x & ((1u64 << bits) - 1)) << (bits * i))
}

fn sx(v: u64, bits: u32) -> i64 {
    ((v << (64 - bits)) as i64) >> (64 - bits)
}

fn sat(v: i64, lo: i64, hi: i64) -> i64 {
    v.clamp(lo, hi)
}

fn lanes(a: u64, b: u64, bits: u32, f: impl Fn(u64, u64, i64, i64) -> u64) -> u64 {
    let mut r = 0;
    for i in 0..64 / bits {
        let (x, y) = (lane(a, bits, i), lane(b, bits, i));
        r = put(r, bits, i, f(x, y, sx(x, bits), sx(y, bits)));
    }
    r
}

macro_rules! op {
    ($name:ident, $bits:expr, |$x:ident, $y:ident, $sx:ident, $sy:ident| $e:expr) => {
        #[allow(unused_variables)]
        pub fn $name(a: u64, b: u64) -> u64 {
            lanes(a, b, $bits, |$x, $y, $sx, $sy| $e)
        }
    };
}

op!(mmx_paddb, 8, |x, y, sx, sy| x.wrapping_add(y));
op!(mmx_paddw, 16, |x, y, sx, sy| x.wrapping_add(y));
op!(mmx_paddd, 32, |x, y, sx, sy| x.wrapping_add(y));
op!(mmx_paddsb, 8, |x, y, sx, sy| sat(sx + sy, -128, 127) as u64);
op!(mmx_paddsw, 16, |x, y, sx, sy| sat(sx + sy, -32768, 32767)
    as u64);
op!(mmx_paddusb, 8, |x, y, sx, sy| sat((x + y) as i64, 0, 255)
    as u64);
op!(
    mmx_paddusw,
    16,
    |x, y, sx, sy| sat((x + y) as i64, 0, 65535) as u64
);
op!(mmx_psubb, 8, |x, y, sx, sy| x.wrapping_sub(y));
op!(mmx_psubw, 16, |x, y, sx, sy| x.wrapping_sub(y));
op!(mmx_psubd, 32, |x, y, sx, sy| x.wrapping_sub(y));
op!(mmx_psubsb, 8, |x, y, sx, sy| sat(sx - sy, -128, 127) as u64);
op!(mmx_psubsw, 16, |x, y, sx, sy| sat(sx - sy, -32768, 32767)
    as u64);
op!(
    mmx_psubusb,
    8,
    |x, y, sx, sy| sat(x as i64 - y as i64, 0, 255) as u64
);
op!(
    mmx_psubusw,
    16,
    |x, y, sx, sy| sat(x as i64 - y as i64, 0, 65535) as u64
);
op!(mmx_pmullw, 16, |x, y, sx, sy| (sx * sy) as u64);
op!(mmx_pmulhw, 16, |x, y, sx, sy| ((sx * sy) >> 16) as u64);
op!(mmx_pcmpeqb, 8, |x, y, sx, sy| if x == y { 0xff } else { 0 });
op!(mmx_pcmpeqw, 16, |x, y, sx, sy| if x == y {
    0xffff
} else {
    0
});
op!(mmx_pcmpeqd, 32, |x, y, sx, sy| if x == y {
    0xffff_ffff
} else {
    0
});
op!(mmx_pcmpgtb, 8, |x, y, sx, sy| if sx > sy {
    0xff
} else {
    0
});
op!(mmx_pcmpgtw, 16, |x, y, sx, sy| if sx > sy {
    0xffff
} else {
    0
});
op!(mmx_pcmpgtd, 32, |x, y, sx, sy| if sx > sy {
    0xffff_ffff
} else {
    0
});

pub fn mmx_pmaddwd(a: u64, b: u64) -> u64 {
    let mut r = 0;
    for i in 0..2 {
        let s = sx(lane(a, 16, 2 * i), 16) * sx(lane(b, 16, 2 * i), 16)
            + sx(lane(a, 16, 2 * i + 1), 16) * sx(lane(b, 16, 2 * i + 1), 16);
        r = put(r, 32, i, s as u64);
    }
    r
}

fn pack(a: u64, b: u64, from: u32, to: u32, lo: i64, hi: i64) -> u64 {
    let n = 64 / from;
    let mut r = 0;
    for i in 0..n {
        r = put(r, to, i, sat(sx(lane(a, from, i), from), lo, hi) as u64);
        r = put(r, to, n + i, sat(sx(lane(b, from, i), from), lo, hi) as u64);
    }
    r
}

pub fn mmx_packsswb(a: u64, b: u64) -> u64 {
    pack(a, b, 16, 8, -128, 127)
}
pub fn mmx_packssdw(a: u64, b: u64) -> u64 {
    pack(a, b, 32, 16, -32768, 32767)
}
pub fn mmx_packuswb(a: u64, b: u64) -> u64 {
    pack(a, b, 16, 8, 0, 255)
}

fn unpack(a: u64, b: u64, bits: u32, high: u32) -> u64 {
    let n = 32 / bits;
    let mut r = 0;
    for i in 0..n {
        r = put(r, bits, 2 * i, lane(a, bits, high * n + i));
        r = put(r, bits, 2 * i + 1, lane(b, bits, high * n + i));
    }
    r
}

pub fn mmx_punpcklbw(a: u64, b: u64) -> u64 {
    unpack(a, b, 8, 0)
}
pub fn mmx_punpcklwd(a: u64, b: u64) -> u64 {
    unpack(a, b, 16, 0)
}
pub fn mmx_punpckldq(a: u64, b: u64) -> u64 {
    unpack(a, b, 32, 0)
}
pub fn mmx_punpckhbw(a: u64, b: u64) -> u64 {
    unpack(a, b, 8, 1)
}
pub fn mmx_punpckhwd(a: u64, b: u64) -> u64 {
    unpack(a, b, 16, 1)
}
pub fn mmx_punpckhdq(a: u64, b: u64) -> u64 {
    unpack(a, b, 32, 1)
}

fn shift(a: u64, bits: u32, f: impl Fn(u64) -> u64) -> u64 {
    let mut r = 0;
    for i in 0..64 / bits {
        r = put(r, bits, i, f(lane(a, bits, i)));
    }
    r
}

pub fn mmx_psllw(a: u64, n: u64) -> u64 {
    shift(a, 16, |x| if n >= 16 { 0 } else { x << n })
}
pub fn mmx_pslld(a: u64, n: u64) -> u64 {
    shift(a, 32, |x| if n >= 32 { 0 } else { x << n })
}
pub fn mmx_psrlw(a: u64, n: u64) -> u64 {
    shift(a, 16, |x| if n >= 16 { 0 } else { x >> n })
}
pub fn mmx_psrld(a: u64, n: u64) -> u64 {
    shift(a, 32, |x| if n >= 32 { 0 } else { x >> n })
}
pub fn mmx_psraw(a: u64, n: u64) -> u64 {
    shift(a, 16, |x| (sx(x, 16) >> n.min(15)) as u64)
}
pub fn mmx_psrad(a: u64, n: u64) -> u64 {
    shift(a, 32, |x| (sx(x, 32) >> n.min(31)) as u64)
}
pub fn mmx_psllq(a: u64, n: u64) -> u64 {
    if n >= 64 {
        0
    } else {
        a << n
    }
}
pub fn mmx_psrlq(a: u64, n: u64) -> u64 {
    if n >= 64 {
        0
    } else {
        a >> n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturating_adds_and_packs() {
        assert_eq!(mmx_paddusb(0xff01, 0x0101), 0xff02);
        assert_eq!(mmx_paddsw(0x7fff, 0x0001), 0x7fff);
        assert_eq!(mmx_packuswb(0x0100_00ff, 0), 0xffff);
        assert_eq!(mmx_punpcklbw(0x0201, 0x0403), 0x0402_0301);
    }
}

// SPDX-License-Identifier: MIT
//! Windows-1252, the ANSI code page of the German and English releases.

const HIGH: [u16; 32] = [
    0x20ac, 0x0081, 0x201a, 0x0192, 0x201e, 0x2026, 0x2020, 0x2021, 0x02c6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008d, 0x017d, 0x008f, 0x0090, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2013, 0x2014,
    0x02dc, 0x2122, 0x0161, 0x203a, 0x0153, 0x009d, 0x017e, 0x0178,
];

pub fn to_unicode(b: u8) -> u16 {
    match b {
        0x80..=0x9f => HIGH[usize::from(b - 0x80)],
        _ => u16::from(b),
    }
}

pub fn from_unicode(u: u16) -> Option<u8> {
    match u {
        0..=0x7f | 0xa0..=0xff => Some(u as u8),
        _ => HIGH.iter().position(|&h| h == u).map(|i| 0x80 + i as u8),
    }
}

/// Guest ANSI bytes as a host string.
pub fn decode(b: &[u8]) -> String {
    b.iter()
        .map(|&x| char::from_u32(u32::from(to_unicode(x))).unwrap_or('?'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn umlauts_and_the_euro_sign() {
        assert_eq!(decode(b"R\xfcckkehr \x80"), "Rückkehr €");
        assert_eq!(from_unicode(0x20ac), Some(0x80));
        assert_eq!(from_unicode(0x3042), None);
    }
}

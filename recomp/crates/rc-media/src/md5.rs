// SPDX-License-Identifier: MIT
//! MD5 (RFC 1321), for the file checksums of Inno Setup 4.2 and InstallShield 6 cabinets.

/// MD5 (RFC 1321).
pub struct Md5 {
    state: [u32; 4],
    buf: Vec<u8>,
    len: u64,
}

impl Md5 {
    pub fn new() -> Self {
        Md5 {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            buf: Vec::with_capacity(64),
            len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if !self.buf.is_empty() {
            let n = (64 - self.buf.len()).min(data.len());
            self.buf.extend_from_slice(&data[..n]);
            data = &data[n..];
            if self.buf.len() == 64 {
                let block: [u8; 64] = self.buf[..].try_into().expect("64");
                self.block(&block);
                self.buf.clear();
            }
        }
        let (blocks, rest) = data.as_chunks::<64>();
        for c in blocks {
            self.block(c);
        }
        self.buf.extend_from_slice(rest);
    }

    fn block(&mut self, b: &[u8; 64]) {
        const S: [u32; 64] = [
            7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20,
            5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
            6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
        ];
        let m: Vec<u32> = b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        let [mut a, mut bb, mut c, mut d] = self.state;
        for (i, &shift) in S.iter().enumerate() {
            let (f, g) = match i / 16 {
                0 => ((bb & c) | (!bb & d), i),
                1 => ((d & bb) | (!d & c), (5 * i + 1) % 16),
                2 => (bb ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (bb | !d), (7 * i) % 16),
            };
            let k = ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32;
            let t = d;
            d = c;
            c = bb;
            bb = bb.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(k)
                    .wrapping_add(m[g])
                    .rotate_left(shift),
            );
            a = t;
        }
        for (s, v) in self.state.iter_mut().zip([a, bb, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    pub fn finish(&self) -> [u8; 16] {
        let mut copy = Md5 {
            state: self.state,
            buf: self.buf.clone(),
            len: self.len,
        };
        let bits = copy.len.wrapping_mul(8);
        let mut pad = vec![0x80u8];
        while (copy.buf.len() + pad.len()) % 64 != 56 {
            pad.push(0);
        }
        pad.extend(bits.to_le_bytes());
        let len = copy.len;
        copy.update(&pad);
        copy.len = len;
        let mut out = [0u8; 16];
        for (i, s) in copy.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
        }
        out
    }
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_values() {
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut m = Md5::new();
        assert_eq!(hex(m.finish()), "d41d8cd98f00b204e9800998ecf8427e");
        m.update(b"The quick brown fox jumps over the lazy dog");
        assert_eq!(hex(m.finish()), "9e107d9d372bb6826bd81d3542a419d6");
        let mut parts = Md5::new();
        for c in vec![b'a'; 1000].chunks(77) {
            parts.update(c);
        }
        let mut whole = Md5::new();
        whole.update(&[b'a'; 1000]);
        assert_eq!(parts.finish(), whole.finish());
    }
}

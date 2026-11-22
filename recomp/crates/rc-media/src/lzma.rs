// SPDX-License-Identifier: MIT
//! Streaming LZMA1 and LZMA2 decoder (raw streams, as embedded in Inno Setup installers).
//!
//! Written from the public LZMA specification (the LZMA SDK's `lzma-specification.txt`).
//! The dictionary grows with the output up to the dictionary size, so memory use is
//! `min(output, dictionary)`. Output is pushed to a [`Write`] sink in large pieces.

use crate::{format_err, Error, Result};
use std::io::{Read, Write};

const PROB_INIT: u16 = 1024;
const TOP: u32 = 1 << 24;
const NUM_STATES: usize = 12;
const POS_STATES_MAX: usize = 16;
const END_POS_MODEL_INDEX: u32 = 14;
const NUM_FULL_DISTANCES: usize = 128;
const OUT_FLUSH: usize = 1 << 20;

/// Byte source for the range decoder, with a running count for LZMA2 chunk bounds.
struct Input<'a> {
    r: &'a mut dyn Read,
    buf: Vec<u8>,
    pos: usize,
    len: usize,
    consumed: u64,
}

impl<'a> Input<'a> {
    fn new(r: &'a mut dyn Read) -> Self {
        Input {
            r,
            buf: vec![0u8; 1 << 16],
            pos: 0,
            len: 0,
            consumed: 0,
        }
    }

    #[inline]
    fn byte(&mut self) -> Result<u8> {
        if self.pos == self.len {
            self.len = self.r.read(&mut self.buf)?;
            self.pos = 0;
            if self.len == 0 {
                return Err(Error::Io(std::io::ErrorKind::UnexpectedEof.into()));
            }
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        self.consumed += 1;
        Ok(b)
    }

    fn be16(&mut self) -> Result<u16> {
        Ok(u16::from(self.byte()?) << 8 | u16::from(self.byte()?))
    }
}

struct RangeDecoder {
    range: u32,
    code: u32,
}

impl RangeDecoder {
    fn new(inp: &mut Input<'_>) -> Result<Self> {
        if inp.byte()? != 0 {
            return format_err("LZMA: bad range coder start byte");
        }
        let mut code = 0u32;
        for _ in 0..4 {
            code = code << 8 | u32::from(inp.byte()?);
        }
        if code == u32::MAX {
            return format_err("LZMA: corrupted range coder");
        }
        Ok(RangeDecoder {
            range: u32::MAX,
            code,
        })
    }

    #[inline]
    fn normalize(&mut self, inp: &mut Input<'_>) -> Result<()> {
        if self.range < TOP {
            self.range <<= 8;
            self.code = self.code << 8 | u32::from(inp.byte()?);
        }
        Ok(())
    }

    #[inline]
    fn bit(&mut self, prob: &mut u16, inp: &mut Input<'_>) -> Result<u32> {
        let bound = (self.range >> 11) * u32::from(*prob);
        let bit = if self.code < bound {
            *prob += (2048 - *prob) >> 5;
            self.range = bound;
            0
        } else {
            *prob -= *prob >> 5;
            self.code -= bound;
            self.range -= bound;
            1
        };
        self.normalize(inp)?;
        Ok(bit)
    }

    fn direct(&mut self, n: u32, inp: &mut Input<'_>) -> Result<u32> {
        let mut res = 0u32;
        for _ in 0..n {
            self.range >>= 1;
            self.code = self.code.wrapping_sub(self.range);
            let t = 0u32.wrapping_sub(self.code >> 31);
            self.code = self.code.wrapping_add(self.range & t);
            if self.code == self.range {
                return format_err("LZMA: corrupted direct bits");
            }
            self.normalize(inp)?;
            res = (res << 1).wrapping_add(t.wrapping_add(1));
        }
        Ok(res)
    }

    fn tree(&mut self, probs: &mut [u16], bits: u32, inp: &mut Input<'_>) -> Result<u32> {
        let mut m = 1usize;
        for _ in 0..bits {
            m = (m << 1) + self.bit(&mut probs[m], inp)? as usize;
        }
        Ok(m as u32 - (1 << bits))
    }

    fn tree_reverse(&mut self, probs: &mut [u16], bits: u32, inp: &mut Input<'_>) -> Result<u32> {
        let mut m = 1usize;
        let mut sym = 0u32;
        for i in 0..bits {
            let b = self.bit(&mut probs[m], inp)?;
            m = (m << 1) + b as usize;
            sym |= b << i;
        }
        Ok(sym)
    }
}

/// Sliding dictionary that also batches output for the sink.
struct Window<'w> {
    buf: Vec<u8>,
    size: usize,
    pos: usize,
    full: bool,
    total: u64,
    out: Vec<u8>,
    sink: &'w mut dyn Write,
}

impl<'w> Window<'w> {
    fn new(dict_size: u32, sink: &'w mut dyn Write) -> Self {
        Window {
            buf: Vec::new(),
            size: (dict_size as usize).max(4096),
            pos: 0,
            full: false,
            total: 0,
            out: Vec::with_capacity(OUT_FLUSH),
            sink,
        }
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.pos = 0;
        self.full = false;
    }

    #[inline]
    fn put(&mut self, b: u8) -> Result<()> {
        if self.full {
            self.buf[self.pos] = b;
            self.pos += 1;
            if self.pos == self.size {
                self.pos = 0;
            }
        } else {
            self.buf.push(b);
            if self.buf.len() == self.size {
                self.full = true;
                self.pos = 0;
            }
        }
        self.total += 1;
        self.out.push(b);
        if self.out.len() >= OUT_FLUSH {
            self.flush()?;
        }
        Ok(())
    }

    /// Byte `dist` positions back (1 = the previous byte).
    #[inline]
    fn get(&self, dist: usize) -> u8 {
        if self.full {
            let i = if dist <= self.pos {
                self.pos - dist
            } else {
                self.size + self.pos - dist
            };
            self.buf[i]
        } else {
            self.buf[self.buf.len() - dist]
        }
    }

    fn has(&self, dist: usize) -> bool {
        dist >= 1 && (self.full || dist <= self.buf.len())
    }

    fn is_empty(&self) -> bool {
        !self.full && self.buf.is_empty()
    }

    fn flush(&mut self) -> Result<()> {
        if !self.out.is_empty() {
            self.sink.write_all(&self.out)?;
            self.out.clear();
        }
        Ok(())
    }
}

struct LenDecoder {
    choice: u16,
    choice2: u16,
    low: [[u16; 8]; POS_STATES_MAX],
    mid: [[u16; 8]; POS_STATES_MAX],
    high: [u16; 256],
}

impl LenDecoder {
    fn new() -> Self {
        LenDecoder {
            choice: PROB_INIT,
            choice2: PROB_INIT,
            low: [[PROB_INIT; 8]; POS_STATES_MAX],
            mid: [[PROB_INIT; 8]; POS_STATES_MAX],
            high: [PROB_INIT; 256],
        }
    }

    fn decode(&mut self, rc: &mut RangeDecoder, ps: usize, inp: &mut Input<'_>) -> Result<u32> {
        if rc.bit(&mut self.choice, inp)? == 0 {
            return rc.tree(&mut self.low[ps], 3, inp);
        }
        if rc.bit(&mut self.choice2, inp)? == 0 {
            return Ok(8 + rc.tree(&mut self.mid[ps], 3, inp)?);
        }
        Ok(16 + rc.tree(&mut self.high, 8, inp)?)
    }
}

#[derive(Clone, Copy)]
struct Props {
    lc: u32,
    lp: u32,
    pb: u32,
}

impl Props {
    fn from_byte(d: u8) -> Result<Self> {
        if d >= 9 * 5 * 5 {
            return format_err("LZMA: bad properties byte");
        }
        let d = u32::from(d);
        Ok(Props {
            lc: d % 9,
            lp: (d / 9) % 5,
            pb: d / 45,
        })
    }
}

/// LZMA coder state: probabilities, reps and the state machine.
struct Lzma {
    props: Props,
    literal: Vec<u16>,
    pos_slot: [[u16; 64]; 4],
    pos_decoders: [u16; 1 + NUM_FULL_DISTANCES - END_POS_MODEL_INDEX as usize],
    align: [u16; 16],
    is_match: [u16; NUM_STATES << 4],
    is_rep: [u16; NUM_STATES],
    is_rep_g0: [u16; NUM_STATES],
    is_rep_g1: [u16; NUM_STATES],
    is_rep_g2: [u16; NUM_STATES],
    is_rep0_long: [u16; NUM_STATES << 4],
    len: LenDecoder,
    rep_len: LenDecoder,
    state: usize,
    rep: [u32; 4],
}

enum Stop {
    /// The requested number of bytes was produced.
    Size,
    /// End-of-stream marker.
    Marker,
}

impl Lzma {
    fn new(props: Props) -> Self {
        Lzma {
            props,
            literal: vec![PROB_INIT; 0x300 << (props.lc + props.lp)],
            pos_slot: [[PROB_INIT; 64]; 4],
            pos_decoders: [PROB_INIT; 1 + NUM_FULL_DISTANCES - END_POS_MODEL_INDEX as usize],
            align: [PROB_INIT; 16],
            is_match: [PROB_INIT; NUM_STATES << 4],
            is_rep: [PROB_INIT; NUM_STATES],
            is_rep_g0: [PROB_INIT; NUM_STATES],
            is_rep_g1: [PROB_INIT; NUM_STATES],
            is_rep_g2: [PROB_INIT; NUM_STATES],
            is_rep0_long: [PROB_INIT; NUM_STATES << 4],
            len: LenDecoder::new(),
            rep_len: LenDecoder::new(),
            state: 0,
            rep: [0; 4],
        }
    }

    fn distance(&mut self, rc: &mut RangeDecoder, len: u32, inp: &mut Input<'_>) -> Result<u32> {
        let len_state = len.min(3) as usize;
        let slot = rc.tree(&mut self.pos_slot[len_state], 6, inp)?;
        if slot < 4 {
            return Ok(slot);
        }
        let direct = (slot >> 1) - 1;
        let mut dist = (2 | (slot & 1)) << direct;
        if slot < END_POS_MODEL_INDEX {
            let base = (dist - slot) as usize;
            dist += rc.tree_reverse(&mut self.pos_decoders[base..], direct, inp)?;
        } else {
            dist = dist.wrapping_add(rc.direct(direct - 4, inp)? << 4);
            dist = dist.wrapping_add(rc.tree_reverse(&mut self.align, 4, inp)?);
        }
        Ok(dist)
    }

    /// Decode until `limit` more bytes are produced (or the end marker, if allowed).
    fn run(
        &mut self,
        rc: &mut RangeDecoder,
        win: &mut Window<'_>,
        inp: &mut Input<'_>,
        limit: Option<u64>,
        dict_size: u32,
    ) -> Result<Stop> {
        let end = limit.map(|l| win.total + l);
        let pb_mask = (1u64 << self.props.pb) - 1;
        let lp_mask = (1u64 << self.props.lp) - 1;
        loop {
            if end.is_some_and(|e| win.total >= e) {
                return Ok(Stop::Size);
            }
            let ps = (win.total & pb_mask) as usize;
            let st = self.state;
            if rc.bit(&mut self.is_match[(st << 4) + ps], inp)? == 0 {
                let prev = if win.is_empty() { 0 } else { win.get(1) };
                let lit_state = (((win.total & lp_mask) as usize) << self.props.lc)
                    + (usize::from(prev) >> (8 - self.props.lc));
                let probs = &mut self.literal[0x300 * lit_state..0x300 * (lit_state + 1)];
                let mut sym = 1usize;
                if st >= 7 {
                    let mut match_byte = usize::from(win.get(self.rep[0] as usize + 1));
                    while sym < 0x100 {
                        let match_bit = (match_byte >> 7) & 1;
                        match_byte <<= 1;
                        let b = rc.bit(&mut probs[((1 + match_bit) << 8) + sym], inp)? as usize;
                        sym = (sym << 1) | b;
                        if match_bit != b {
                            break;
                        }
                    }
                }
                while sym < 0x100 {
                    sym = (sym << 1) | rc.bit(&mut probs[sym], inp)? as usize;
                }
                win.put((sym - 0x100) as u8)?;
                self.state = if st < 4 {
                    0
                } else if st < 10 {
                    st - 3
                } else {
                    st - 6
                };
                continue;
            }

            let len;
            if rc.bit(&mut self.is_rep[st], inp)? != 0 {
                if win.is_empty() {
                    return format_err("LZMA: rep match at stream start");
                }
                if rc.bit(&mut self.is_rep_g0[st], inp)? == 0 {
                    if rc.bit(&mut self.is_rep0_long[(st << 4) + ps], inp)? == 0 {
                        self.state = if st < 7 { 9 } else { 11 };
                        let b = win.get(self.rep[0] as usize + 1);
                        win.put(b)?;
                        continue;
                    }
                } else {
                    let dist;
                    if rc.bit(&mut self.is_rep_g1[st], inp)? == 0 {
                        dist = self.rep[1];
                    } else {
                        if rc.bit(&mut self.is_rep_g2[st], inp)? == 0 {
                            dist = self.rep[2];
                        } else {
                            dist = self.rep[3];
                            self.rep[3] = self.rep[2];
                        }
                        self.rep[2] = self.rep[1];
                    }
                    self.rep[1] = self.rep[0];
                    self.rep[0] = dist;
                }
                len = self.rep_len.decode(rc, ps, inp)?;
                self.state = if st < 7 { 8 } else { 11 };
            } else {
                self.rep[3] = self.rep[2];
                self.rep[2] = self.rep[1];
                self.rep[1] = self.rep[0];
                len = self.len.decode(rc, ps, inp)?;
                self.state = if st < 7 { 7 } else { 10 };
                let d = self.distance(rc, len, inp)?;
                if d == u32::MAX {
                    return Ok(Stop::Marker);
                }
                self.rep[0] = d;
                if d >= dict_size || !win.has(d as usize + 1) {
                    return format_err("LZMA: match distance outside the dictionary");
                }
            }
            let mut n = u64::from(len) + 2;
            if let Some(e) = end {
                if win.total + n > e {
                    // A match may not run past the declared size.
                    n = e - win.total;
                }
            }
            let dist = self.rep[0] as usize + 1;
            for _ in 0..n {
                let b = win.get(dist);
                win.put(b)?;
            }
        }
    }
}

/// Raw LZMA1 stream: 5 header bytes (properties, dictionary size LE) followed by data.
/// With `size = Some(n)` decoding stops after n bytes. With `None` it runs to the end
/// marker, or, for streams without one, until the input is exhausted (the Inno setup
/// blocks); output produced before running out of input is kept.
pub fn decode_lzma1(
    input: &mut dyn Read,
    output: &mut dyn Write,
    size: Option<u64>,
) -> Result<u64> {
    let mut inp = Input::new(input);
    let props = Props::from_byte(inp.byte()?)?;
    let mut d = [0u8; 4];
    for b in &mut d {
        *b = inp.byte()?;
    }
    let dict_size = u32::from_le_bytes(d).max(4096);
    let mut win = Window::new(dict_size, output);
    let mut lz = Lzma::new(props);
    let result = RangeDecoder::new(&mut inp)
        .and_then(|mut rc| lz.run(&mut rc, &mut win, &mut inp, size, dict_size));
    match result {
        Ok(_) => {}
        Err(Error::Io(e)) if size.is_none() && e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Err(e) => return Err(e),
    }
    win.flush()?;
    Ok(win.total)
}

/// LZMA2 dictionary size from its one-byte property.
pub fn lzma2_dict_size(p: u8) -> Result<u32> {
    match p {
        0..=39 => Ok((2 | u32::from(p & 1)) << (p / 2 + 11)),
        40 => Ok(u32::MAX),
        _ => format_err("LZMA2: bad dictionary property"),
    }
}

/// Raw LZMA2 stream preceded by its one-byte dictionary property (Inno's layout).
/// Stops at the end chunk, or as soon as `size` bytes (if given) are produced.
pub fn decode_lzma2(
    input: &mut dyn Read,
    output: &mut dyn Write,
    size: Option<u64>,
) -> Result<u64> {
    let mut inp = Input::new(input);
    let dict_size = lzma2_dict_size(inp.byte()?)?;
    // An "unlimited" dictionary never needs more than the output itself.
    let win_size = match size {
        Some(s) => dict_size.min(u32::try_from(s).unwrap_or(u32::MAX)),
        None => dict_size,
    };
    let mut win = Window::new(win_size, output);
    let mut lz: Option<Lzma> = None;
    let mut need_dict_reset = true;
    loop {
        if size.is_some_and(|s| win.total >= s) {
            break;
        }
        let control = inp.byte()?;
        if control == 0 {
            break;
        }
        if control == 1 || control == 2 {
            if control == 1 {
                win.reset();
                need_dict_reset = false;
            } else if need_dict_reset {
                return format_err("LZMA2: missing dictionary reset");
            }
            let n = u32::from(inp.be16()?) + 1;
            for _ in 0..n {
                let b = inp.byte()?;
                win.put(b)?;
            }
            continue;
        }
        if control < 0x80 {
            return format_err(format!("LZMA2: bad control byte {control:#x}"));
        }
        let unpacked = (u64::from(control & 0x1f) << 16) + u64::from(inp.be16()?) + 1;
        let packed = u64::from(inp.be16()?) + 1;
        let reset = (control >> 5) & 3;
        if reset == 3 {
            win.reset();
            need_dict_reset = false;
        } else if need_dict_reset {
            return format_err("LZMA2: missing dictionary reset");
        }
        if reset >= 2 {
            let props = Props::from_byte(inp.byte()?)?;
            if props.lc + props.lp > 4 {
                return format_err("LZMA2: lc + lp > 4");
            }
            lz = Some(Lzma::new(props));
        } else if let Some(l) = lz.as_mut() {
            if reset == 1 {
                *l = Lzma::new(l.props);
            }
        } else {
            return format_err("LZMA2: chunk without properties");
        }
        let l = lz.as_mut().expect("properties set");
        let start = inp.consumed;
        let mut rc = RangeDecoder::new(&mut inp)?;
        l.run(&mut rc, &mut win, &mut inp, Some(unpacked), win_size.max(1))?;
        if inp.consumed - start != packed {
            return format_err("LZMA2: chunk packed size mismatch");
        }
    }
    win.flush()?;
    Ok(win.total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_sizes() {
        assert_eq!(lzma2_dict_size(0).unwrap(), 4096);
        assert_eq!(lzma2_dict_size(1).unwrap(), 6144);
        assert_eq!(lzma2_dict_size(16).unwrap(), 1 << 20);
        assert_eq!(lzma2_dict_size(8).unwrap(), 1 << 16);
        assert_eq!(lzma2_dict_size(40).unwrap(), u32::MAX);
        assert!(lzma2_dict_size(41).is_err());
    }

    #[test]
    fn lzma2_uncompressed_chunks() {
        // dict prop, chunk "hello" (reset), chunk " world" (no reset), end.
        let mut s = vec![18u8, 1, 0, 4];
        s.extend_from_slice(b"hello");
        s.extend_from_slice(&[2, 0, 5]);
        s.extend_from_slice(b" world");
        s.push(0);
        let mut out = Vec::new();
        assert_eq!(decode_lzma2(&mut &s[..], &mut out, None).unwrap(), 11);
        assert_eq!(out, b"hello world");
        // Size limit stops early, even mid-stream.
        let mut out = Vec::new();
        decode_lzma2(&mut &s[..], &mut out, Some(5)).unwrap();
        assert_eq!(out, b"hello");
    }

    /// LZMA1 stream for "abcabcabcabc\n" (lc=3 lp=0 pb=2, 64 KiB dictionary, end marker):
    /// liblzma raw LZMA1 output with the 5-byte properties header prepended.
    const LZMA1_ABC: &[u8] = &[
        0x5d, 0x00, 0x00, 0x01, 0x00, 0x00, 0x30, 0x98, 0x88, 0xa7, 0xea, 0x31, 0xa2, 0xb0, 0xcf,
        0xff, 0xff, 0xb0, 0xb2, 0x00, 0x00,
    ];

    /// LZMA2 stream for "abc" x 100 + "xyz" (64 KiB dictionary): liblzma raw LZMA2 output
    /// with Inno's one-byte dictionary property (8 = 64 KiB) prepended.
    const LZMA2_ABC: &[u8] = &[
        0x08, 0xe0, 0x01, 0x2e, 0x00, 0x0e, 0x5d, 0x00, 0x30, 0x98, 0x88, 0xad, 0x4b, 0x36, 0xed,
        0x24, 0xb9, 0xc4, 0x33, 0xfc, 0xb4, 0xfc, 0x00,
    ];

    #[test]
    fn lzma1_reference_vector() {
        let mut out = Vec::new();
        assert_eq!(
            decode_lzma1(&mut &LZMA1_ABC[..], &mut out, None).unwrap(),
            13
        );
        assert_eq!(out, b"abcabcabcabc\n");
        let mut out = Vec::new();
        decode_lzma1(&mut &LZMA1_ABC[..], &mut out, Some(5)).unwrap();
        assert_eq!(out, b"abcab");
    }

    #[test]
    fn lzma2_reference_vector() {
        let mut expected = b"abc".repeat(100);
        expected.extend_from_slice(b"xyz");
        let mut out = Vec::new();
        assert_eq!(
            decode_lzma2(&mut &LZMA2_ABC[..], &mut out, None).unwrap(),
            303
        );
        assert_eq!(out, expected);
    }
}

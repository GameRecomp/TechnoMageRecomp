// SPDX-License-Identifier: MIT
//! Function-level comparison of two builds (research question 5: how much code differs
//! between the three Cultures 1 executables).
//!
//! Each function gets a fingerprint over its normalised instructions: opcodes, registers and
//! small constants are kept, while absolute addresses inside the image and call targets are
//! masked, and branches inside the function are kept as offsets from its entry. Two functions
//! with the same fingerprint are the same code, only placed elsewhere.

use crate::codemap::CodeMap;
use crate::image::Image;
use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind};
use std::collections::HashMap;

/// FNV-1a, 64 bit.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

fn add_insn(h: &mut Fnv, img: &Image, i: &Instruction, entry: u32, inside: &dyn Fn(u32) -> bool) {
    let addr = |v: u32| img.contains(v);
    h.add(i.code() as u64);
    h.add(i.segment_prefix() as u64);
    for op in 0..i.op_count() {
        let k = i.op_kind(op);
        h.add(k as u64);
        match k {
            OpKind::Register => h.add(i.op_register(op) as u64),
            OpKind::NearBranch32 | OpKind::NearBranch16 => {
                let t = i.near_branch_target() as u32;
                h.add(if inside(t) {
                    u64::from(t.wrapping_sub(entry))
                } else {
                    0
                });
            }
            OpKind::Immediate32 => {
                let v = i.immediate32();
                h.add(if addr(v) { 1 << 40 } else { u64::from(v) });
            }
            OpKind::Memory => {
                h.add(i.memory_base() as u64);
                h.add(i.memory_index() as u64);
                h.add(u64::from(i.memory_index_scale()));
                h.add(i.memory_size() as u64);
                let d = i.memory_displacement32();
                h.add(if addr(d) { 1 << 40 } else { u64::from(d) });
            }
            _ => h.add(i.immediate(op)),
        }
    }
}

/// `(entry, fingerprint, size)` for every function of a code map.
pub fn fingerprints(img: &Image, map: &CodeMap) -> Vec<(u32, u64, u32)> {
    let mut out = Vec::with_capacity(map.functions.len());
    for f in &map.functions {
        let entry = f.entry.0;
        let inside = |t: u32| f.chunks.iter().any(|c| t >= c[0].0 && t < c[1].0);
        let mut h = Fnv::new();
        for c in &f.chunks {
            let (start, end) = (c[0].0, c[1].0);
            h.add(u64::from(start.wrapping_sub(entry)));
            let Some(bytes) = img.tail(start) else {
                continue;
            };
            let len = ((end - start) as usize).min(bytes.len());
            let mut d = Decoder::with_ip(32, &bytes[..len], u64::from(start), DecoderOptions::NONE);
            let mut i = Instruction::default();
            while d.can_decode() {
                d.decode_out(&mut i);
                add_insn(&mut h, img, &i, entry, &inside);
            }
        }
        out.push((entry, h.0, f.size));
    }
    out
}

/// Result of comparing two builds.
#[derive(Debug, Clone, Default)]
pub struct Diff {
    pub a_functions: usize,
    pub b_functions: usize,
    pub a_bytes: u64,
    pub b_bytes: u64,
    /// Functions of A with an identical counterpart in B.
    pub identical: usize,
    pub identical_bytes: u64,
    /// Entries of A without counterpart in B, and vice versa.
    pub a_only: Vec<(u32, u32)>,
    pub b_only: Vec<(u32, u32)>,
}

/// Match functions by fingerprint (as a multiset).
pub fn diff(a: &[(u32, u64, u32)], b: &[(u32, u64, u32)]) -> Diff {
    let mut pool: HashMap<u64, Vec<(u32, u32)>> = HashMap::new();
    for &(va, h, size) in b {
        pool.entry(h).or_default().push((va, size));
    }
    let mut d = Diff {
        a_functions: a.len(),
        b_functions: b.len(),
        a_bytes: a.iter().map(|f| u64::from(f.2)).sum(),
        b_bytes: b.iter().map(|f| u64::from(f.2)).sum(),
        ..Diff::default()
    };
    for &(va, h, size) in a {
        match pool.get_mut(&h).and_then(Vec::pop) {
            Some(_) => {
                d.identical += 1;
                d.identical_bytes += u64::from(size);
            }
            None => d.a_only.push((va, size)),
        }
    }
    d.b_only = pool.into_values().flatten().collect();
    d.b_only.sort_unstable();
    d
}

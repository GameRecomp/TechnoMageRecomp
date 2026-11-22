// SPDX-License-Identifier: MIT
//! x86 semantics: decoded instructions to IR.
//!
//! [`lift_function`] lifts one function of a code map. [`lift_insn`] lifts
//! one instruction; how control leaves it comes from discovery's
//! [`rc_discover::Flow`], which already resolved import slots and jump
//! tables.
//!
//! Integer instructions are modelled exactly, flags included (flags that
//! x86 leaves undefined get some value). An instruction this crate does not
//! model becomes a trap naming it, so a whole program always lifts and the
//! report says what is missing.
//!
//! Shared with TropicoRecomp (same semantics, same IR); only the code map it
//! reads is this project's.

mod sem;

use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter};
use rc_discover::{CodeMap, Flow, Function, Image};
use rc_ir as ir;

pub use sem::lift_insn;

/// Lift one function of the code map.
pub fn lift_function(image: &Image, map: &CodeMap, f: &Function) -> ir::Function {
    lift_entry(image, map, f, f.entry.0)
}

/// Lift the body of `f` as a function entered at `entry`: `f`'s own entry, or a
/// label inside it (an address the program takes as a value, such as an SEH
/// handler), which then gets a C function of its own. Execution starts at
/// `entry` even when the body has code below it.
pub fn lift_entry(image: &Image, map: &CodeMap, f: &Function, entry: u32) -> ir::Function {
    let mut fmt = IntelFormatter::new();
    let mut insns = Vec::with_capacity(f.body.len());
    let mut labels = std::collections::BTreeSet::new();
    labels.insert(entry);
    for &a in &f.body {
        let Some(info) = map.insns.get(&a) else {
            continue;
        };
        let ins = decode(image, a, info.len);
        let table = match info.flow {
            Flow::JumpTable => map
                .jump_table(a)
                .map(|t| t.targets.iter().map(|v| v.0).collect::<Vec<u32>>()),
            _ => None,
        };
        let mut text = String::new();
        fmt.format(&ins, &mut text);
        let mut li = lift_insn(&ins, info.flow, table.as_deref());
        li.text = text;
        match &li.term {
            ir::Term::Jump(t) => {
                labels.insert(*t);
            }
            ir::Term::Branch(_, t) => {
                labels.insert(*t);
                labels.insert(li.next());
            }
            ir::Term::JumpTable { cases, .. } => labels.extend(cases.iter().copied()),
            _ => {}
        }
        insns.push(li);
    }
    ir::Function {
        addr: entry,
        insns,
        labels: labels.into_iter().collect(),
        ..Default::default()
    }
}

/// Decode the instruction at `va`.
pub fn decode(image: &Image, va: u32, len: u8) -> Instruction {
    let bytes = image
        .tail(va)
        .map_or(&[][..], |t| &t[..t.len().min(usize::from(len))]);
    let mut d = Decoder::with_ip(32, bytes, u64::from(va), DecoderOptions::NONE);
    d.decode()
}

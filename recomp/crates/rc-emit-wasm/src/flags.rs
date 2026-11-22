// SPDX-License-Identifier: MIT
//! Which flag settings a function needs: a backward liveness analysis over its instructions.
//!
//! The lifter sets every arithmetic flag an instruction defines, and most are never read: the
//! next instruction sets them again. A C compiler or Cranelift drops those settings as dead
//! code. A browser cannot: the blocks of a function are the cases of one `br_table` loop, so a
//! flag's local seems to flow from every block into every other. The settings are dropped here
//! instead, which leaves the code a third smaller where it computes, and faster.
//!
//! A flag is live where a later read can see it: an expression that reads it, or a place where
//! the back end stores the flags into `c` (a trap, a tail call, a patch point, an indirect jump,
//! a return with `flags_at_return`). A call passes the flags' locals through unchanged, or,
//! with `flags_at_return`, loads them from `c` after it. Only `SetFlag`s at the top level of an
//! instruction are dropped; settings inside an `If` or a `Loop` are kept and do not end a flag's
//! life (they may not run). The direction flag lives in `c` and is left alone.

use std::collections::BTreeMap;

use rc_ir::{Expr as E, Flag, Function, Stmt as S, Term};

/// The six arithmetic flags as bits (`flag_index` order).
type Set = u8;
const ALL: Set = 0x3f;

fn bit(f: Flag) -> Set {
    crate::flag_index(f).map_or(0, |k| 1 << k)
}

fn expr(e: &E) -> Set {
    match e {
        E::Flag(f) => bit(*f),
        E::Const(..)
        | E::Tmp(..)
        | E::Reg(_)
        | E::FsBase
        | E::FpuStatus
        | E::FpuControl
        | E::Mm(_) => 0,
        E::Load(_, a) | E::Un(_, _, a) | E::Zext(_, a) | E::Sext(_, _, a) | E::Trunc(_, a) => {
            expr(a)
        }
        E::Bin(_, _, a, b) | E::Mmx(_, a, b) => expr(a) | expr(b),
        E::Select(_, c, a, b) => expr(c) | expr(a) | expr(b),
    }
}

/// Everything statements read, nested ones included (none of their settings counted).
fn reads(body: &[S]) -> Set {
    body.iter().fold(0, |acc, s| {
        acc | match s {
            S::Let(_, e) | S::SetReg(_, e) | S::SetFlag(_, e) | S::SetMm(_, e) | S::BreakIf(e) => {
                expr(e)
            }
            S::Store(a, v) => expr(a) | expr(v),
            S::If(c, b) => expr(c) | reads(b),
            S::Loop(b) => reads(b),
            S::Fpu(op) => crate::fpu_encode(op).1.map_or(0, expr),
            S::Helper(_) => 0,
            // Both store the flags into `c`.
            S::Point(_) | S::Trap(_) => ALL,
        }
    })
}

/// For every instruction of `f`, which of its statements (at the top level) set a flag that is
/// never read: `dead[k][i]` for statement `i` of instruction `k`.
pub fn dead_settings(
    f: &Function,
    is_function: impl Fn(u32) -> bool,
    flags_at_return: bool,
) -> Vec<Vec<bool>> {
    let n = f.insns.len();
    let at: BTreeMap<u32, usize> = f
        .insns
        .iter()
        .enumerate()
        .map(|(k, i)| (i.addr, k))
        .collect();
    let mut live_in: Vec<Set> = vec![0; n];

    // What is live where control goes to `t`: a block of this function, or a tail call or trap
    // (both store the flags).
    let target = |live_in: &[Set], t: u32| at.get(&t).map_or(ALL, |&k| live_in[k]);
    // Live after instruction `k`'s statements: its terminator's needs.
    let after = |live_in: &[Set], k: usize| -> Set {
        let ins = &f.insns[k];
        let next = ins.next();
        let following = f.insns.get(k + 1);
        let fall = |live_in: &[Set]| match following {
            Some(n) if n.addr == next => live_in[k + 1],
            _ => target(live_in, next),
        };
        let call = |live_in: &[Set]| if flags_at_return { 0 } else { fall(live_in) };
        match &ins.term {
            Term::Next => fall(live_in),
            Term::Jump(t) => target(live_in, *t),
            Term::Branch(c, t) => expr(c) | target(live_in, *t) | fall(live_in),
            Term::Call { target: t, .. } => {
                if is_function(*t) {
                    call(live_in)
                } else {
                    ALL // a trap
                }
            }
            Term::CallIndirect { target: t, .. } => expr(t) | call(live_in),
            Term::Ret(_) => {
                if flags_at_return {
                    ALL
                } else {
                    0
                }
            }
            // Both store the flags (the table's trap when no case matches).
            Term::JumpIndirect(_) | Term::JumpTable { .. } => ALL,
            // What follows in the text runs next; at the end the function returns.
            Term::Stop => following.map_or(0, |_| live_in[k + 1]),
        }
    };
    // Backward through one instruction: what is live before it, and its dead settings.
    let through = |k: usize, mut live: Set, dead: &mut Vec<bool>| -> Set {
        let stmts = &f.insns[k].stmts;
        dead.clear();
        dead.resize(stmts.len(), false);
        for (i, s) in stmts.iter().enumerate().rev() {
            match s {
                S::SetFlag(fl, e) if bit(*fl) != 0 => {
                    if live & bit(*fl) == 0 {
                        dead[i] = true;
                    } else {
                        live = (live & !bit(*fl)) | expr(e);
                    }
                }
                s => live |= reads(std::slice::from_ref(s)),
            }
        }
        live
    };

    let mut scratch = Vec::new();
    loop {
        let mut changed = false;
        for k in (0..n).rev() {
            let live = through(k, after(&live_in, k), &mut scratch);
            if live != live_in[k] {
                live_in[k] = live;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    (0..n)
        .map(|k| {
            let mut dead = Vec::new();
            through(k, after(&live_in, k), &mut dead);
            dead
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_ir::{Insn, Reg, W};

    fn insn(addr: u32, stmts: Vec<S>, term: Term) -> Insn {
        Insn {
            addr,
            len: 2,
            text: String::new(),
            stmts,
            term,
        }
    }

    fn set(f: Flag) -> S {
        S::SetFlag(f, E::Reg(Reg::d(0)).trunc(W::B8))
    }

    #[test]
    fn settings_nothing_reads_are_dead() {
        let f = Function {
            addr: 0x1000,
            insns: vec![
                // Overwritten by the next instruction before anything reads them.
                insn(0x1000, vec![set(Flag::Zf), set(Flag::Cf)], Term::Next),
                // ZF is read by the branch; CF is not, and a return would store it.
                insn(
                    0x1002,
                    vec![set(Flag::Zf), set(Flag::Cf)],
                    Term::Branch(E::Flag(Flag::Zf), 0x1008),
                ),
                // A setting inside an If may not run: kept, and CF stays live above it.
                insn(
                    0x1004,
                    vec![S::If(E::Reg(Reg::d(1)), vec![set(Flag::Cf)]), set(Flag::Sf)],
                    Term::Next,
                ),
                insn(0x1006, vec![set(Flag::Sf)], Term::Ret(0)),
                insn(0x1008, vec![], Term::Ret(0)),
            ],
            labels: vec![0x1000, 0x1008],
            except_block: false,
            continuations: Vec::new(),
        };
        let dead = dead_settings(&f, |_| false, true);
        assert_eq!(dead[0], [true, true]);
        assert_eq!(dead[1], [false, false]); // CF reaches the returns (flags_at_return)
        assert_eq!(dead[2], [false, true]);
        assert_eq!(dead[3], [false]);
        // Without the flags at the return, only ZF of the second instruction is read.
        let dead = dead_settings(&f, |_| false, false);
        assert_eq!(dead[1], [false, true]);
        assert_eq!(dead[3], [true]);
    }
}

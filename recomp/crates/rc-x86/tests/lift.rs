// SPDX-License-Identifier: MIT
//! How instructions become IR: terminators, traps and loops. The values the
//! IR computes are checked against the real CPU by rc-difftest.

use iced_x86::{Decoder, DecoderOptions, Instruction};
use rc_discover::Flow;
use rc_ir::{Expr, Stmt, Term};

fn dec(bytes: &[u8]) -> Instruction {
    Decoder::with_ip(32, bytes, 0x0040_1000, DecoderOptions::NONE).decode()
}

fn trap_text(s: &[Stmt]) -> Option<&str> {
    s.iter().find_map(|s| match s {
        Stmt::Trap(t) => Some(t.as_str()),
        _ => None,
    })
}

#[test]
fn calls_and_returns() {
    // call rel32
    let i = rc_x86::lift_insn(&dec(&[0xe8, 0, 0, 0, 0]), Flow::Call(0x0040_1005), None);
    assert_eq!(
        i.term,
        Term::Call {
            target: 0x0040_1005,
            ret: 0x0040_1005
        }
    );
    // ret 8
    let i = rc_x86::lift_insn(&dec(&[0xc2, 8, 0]), Flow::Ret, None);
    assert_eq!(i.term, Term::Ret(8));
    // call [0x0061f4c8]: through the import table
    let i = rc_x86::lift_insn(
        &dec(&[0xff, 0x15, 0xc8, 0xf4, 0x61, 0x00]),
        Flow::CallImport(0x0061_f4c8),
        None,
    );
    assert!(matches!(
        i.term,
        Term::CallIndirect {
            ret: 0x0040_1006,
            ..
        }
    ));
}

#[test]
fn branches_and_tables() {
    // jne +2
    let i = rc_x86::lift_insn(&dec(&[0x75, 0x02]), Flow::Cond(0x0040_1004), None);
    assert!(matches!(i.term, Term::Branch(_, 0x0040_1004)));
    // jmp [eax*4 + 0x401800]
    let t = [0x0040_1010, 0x0040_1020];
    let i = rc_x86::lift_insn(
        &dec(&[0xff, 0x24, 0x85, 0x00, 0x18, 0x40, 0x00]),
        Flow::JumpTable,
        Some(&t),
    );
    match i.term {
        Term::JumpTable { cases, target } => {
            assert_eq!(cases, t);
            assert!(matches!(target, Expr::Tmp(..)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn traps_name_what_is_missing() {
    // addps xmm0, xmm1: SSE is not modelled.
    let i = rc_x86::lift_insn(&dec(&[0x0f, 0x58, 0xc1]), Flow::Next, None);
    assert_eq!(i.term, Term::Stop);
    assert!(trap_text(&i.stmts).is_some_and(|t| t.contains("addps") && t.contains("0x00401000")));
    // paddw mm0, mm1 is: an MMX register write.
    let i = rc_x86::lift_insn(&dec(&[0x0f, 0xfd, 0xc1]), Flow::Next, None);
    assert!(
        matches!(i.stmts.as_slice(), [Stmt::SetMm(0, _)]),
        "{:?}",
        i.stmts
    );
    // emms is: every x87 register tagged empty.
    let i = rc_x86::lift_insn(&dec(&[0x0f, 0x77]), Flow::Next, None);
    assert!(trap_text(&i.stmts).is_none());
    let frees = i
        .stmts
        .iter()
        .filter(|s| matches!(s, Stmt::Fpu(rc_ir::Fpu::Free(_))))
        .count();
    assert_eq!(frees, 8);
    // int3 ends execution with a trap of its own.
    let i = rc_x86::lift_insn(&dec(&[0xcc]), Flow::Stop, None);
    assert_eq!(i.term, Term::Stop);
    assert!(trap_text(&i.stmts).is_some_and(|t| t.starts_with("int3")));
}

#[test]
fn repeated_string_instructions_loop() {
    // rep movsd
    let i = rc_x86::lift_insn(&dec(&[0xf3, 0xa5]), Flow::Next, None);
    assert_eq!(i.term, Term::Next);
    assert!(
        matches!(i.stmts.as_slice(), [Stmt::Loop(body)] if matches!(body.first(), Some(Stmt::BreakIf(_))))
    );
    // A plain movsd has no loop.
    let i = rc_x86::lift_insn(&dec(&[0xa5]), Flow::Next, None);
    assert!(!i.stmts.iter().any(|s| matches!(s, Stmt::Loop(_))));
}

#[test]
fn division_guards_trap() {
    // div ecx
    let i = rc_x86::lift_insn(&dec(&[0xf7, 0xf1]), Flow::Next, None);
    let guards = i
        .stmts
        .iter()
        .filter(|s| matches!(s, Stmt::If(_, b) if matches!(b.as_slice(), [Stmt::Trap(_)])))
        .count();
    assert_eq!(guards, 2, "divide by zero and divide overflow");
}

#[test]
fn segment_registers_follow_the_flat_model() {
    // push ds; pop es: a selector is pushed and dropped again, no trap.
    for code in [&[0x1e][..], &[0x07][..], &[0x8c, 0xd8][..]] {
        let i = rc_x86::lift_insn(&dec(code), Flow::Next, None);
        assert!(trap_text(&i.stmts).is_none(), "{code:02x?}");
    }
    // mov ds, ax still traps: segments cannot be changed.
    let i = rc_x86::lift_insn(&dec(&[0x8e, 0xd8]), Flow::Next, None);
    assert!(trap_text(&i.stmts).is_some());
}

/// Number of guest-memory loads in a statement list (not inside loops or ifs).
fn loads(s: &[Stmt]) -> usize {
    s.iter()
        .filter(|s| matches!(s, Stmt::Let(_, Expr::Load(..))))
        .count()
}

#[test]
fn values_that_are_dropped_are_not_loaded() {
    // popad: seven registers from the stack; the saved esp is skipped, not read.
    let i = rc_x86::lift_insn(&dec(&[0x61]), Flow::Next, None);
    assert_eq!(loads(&i.stmts), 7);
    // pop es: the selector is dropped without a read.
    let i = rc_x86::lift_insn(&dec(&[0x07]), Flow::Next, None);
    assert_eq!(loads(&i.stmts), 0);
}

#[test]
fn compare_with_zero_has_a_constant_carry() {
    // cmp eax, 0: CF is 0, not `eax < 0`.
    let i = rc_x86::lift_insn(&dec(&[0x83, 0xf8, 0x00]), Flow::Next, None);
    let cf = i.stmts.iter().find_map(|s| match s {
        Stmt::SetFlag(rc_ir::Flag::Cf, e) => Some(e),
        _ => None,
    });
    assert_eq!(cf, Some(&Expr::Const(rc_ir::W::B8, 0)));
}

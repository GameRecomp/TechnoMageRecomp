// SPDX-License-Identifier: MIT
//! MMX: eight 64-bit registers of their own (`c->mm`). On a real CPU they share the x87
//! registers; programs separate the two uses with `emms`, so the registers are kept apart
//! here (the x87 side of `emms` is in x87.rs).

use iced_x86::{Instruction, Mnemonic, OpKind, Register};
use rc_ir::{BinOp as B, Expr as E, MmxOp as X, Stmt as S, UnOp as U, W};

use super::{Cx, Loc, Res};

fn mm(r: Register) -> Option<u8> {
    r.is_mm()
        .then(|| (r.number() - Register::MM0.number()) as u8)
}

impl Cx {
    /// An MMX operand's 64-bit value: a register, memory, or an immediate shift count.
    fn mm_src(&mut self, ins: &Instruction, i: u32) -> Res<E> {
        if ins.op_kind(i) == OpKind::Register {
            if let Some(n) = mm(ins.op_register(i)) {
                return Ok(E::Mm(n));
            }
        }
        let l = self.loc(ins, i)?;
        let v = self.get(&l);
        Ok(v.zext(W::B64))
    }

    fn mm_dst(ins: &Instruction) -> Res<u8> {
        if ins.op0_kind() == OpKind::Register {
            if let Some(n) = mm(ins.op0_register()) {
                return Ok(n);
            }
        }
        Err("MMX destination".into())
    }

    /// Lift an MMX instruction; false if `m` is not one.
    pub(super) fn mmx(&mut self, ins: &Instruction, m: Mnemonic) -> Res<bool> {
        use Mnemonic as M;
        let op = match m {
            M::Paddb => X::Paddb,
            M::Paddw => X::Paddw,
            M::Paddd => X::Paddd,
            M::Paddsb => X::Paddsb,
            M::Paddsw => X::Paddsw,
            M::Paddusb => X::Paddusb,
            M::Paddusw => X::Paddusw,
            M::Psubb => X::Psubb,
            M::Psubw => X::Psubw,
            M::Psubd => X::Psubd,
            M::Psubsb => X::Psubsb,
            M::Psubsw => X::Psubsw,
            M::Psubusb => X::Psubusb,
            M::Psubusw => X::Psubusw,
            M::Pmullw => X::Pmullw,
            M::Pmulhw => X::Pmulhw,
            M::Pmaddwd => X::Pmaddwd,
            M::Pcmpeqb => X::Pcmpeqb,
            M::Pcmpeqw => X::Pcmpeqw,
            M::Pcmpeqd => X::Pcmpeqd,
            M::Pcmpgtb => X::Pcmpgtb,
            M::Pcmpgtw => X::Pcmpgtw,
            M::Pcmpgtd => X::Pcmpgtd,
            M::Packsswb => X::Packsswb,
            M::Packssdw => X::Packssdw,
            M::Packuswb => X::Packuswb,
            M::Punpcklbw => X::Punpcklbw,
            M::Punpcklwd => X::Punpcklwd,
            M::Punpckldq => X::Punpckldq,
            M::Punpckhbw => X::Punpckhbw,
            M::Punpckhwd => X::Punpckhwd,
            M::Punpckhdq => X::Punpckhdq,
            M::Psllw => X::Psllw,
            M::Pslld => X::Pslld,
            M::Psllq => X::Psllq,
            M::Psrlw => X::Psrlw,
            M::Psrld => X::Psrld,
            M::Psrlq => X::Psrlq,
            M::Psraw => X::Psraw,
            M::Psrad => X::Psrad,
            M::Pand | M::Pandn | M::Por | M::Pxor | M::Movd | M::Movq => {
                return self.mmx_move_logic(ins, m);
            }
            _ => return Ok(false),
        };
        // Only the MMX forms (SSE2's xmm forms share the mnemonics).
        let d = match Self::mm_dst(ins) {
            Ok(d) => d,
            Err(_) => return Ok(false),
        };
        let src = self.mm_src(ins, 1)?;
        self.stmts
            .push(S::SetMm(d, E::Mmx(op, Box::new(E::Mm(d)), Box::new(src))));
        Ok(true)
    }

    fn mmx_move_logic(&mut self, ins: &Instruction, m: Mnemonic) -> Res<bool> {
        use Mnemonic as M;
        let is_mm = |k: OpKind, r: Register| k == OpKind::Register && r.is_mm();
        let d_mm = is_mm(ins.op0_kind(), ins.op0_register());
        let s_mm = is_mm(ins.op1_kind(), ins.op1_register());
        if !d_mm && !s_mm {
            return Ok(false); /* SSE forms */
        }
        match m {
            M::Pand | M::Pandn | M::Por | M::Pxor => {
                let d = Self::mm_dst(ins)?;
                let src = self.mm_src(ins, 1)?;
                let dst = E::Mm(d);
                let v = match m {
                    M::Pand => E::bin(B::And, W::B64, dst, src),
                    M::Pandn => E::bin(B::And, W::B64, E::un(U::Not, W::B64, dst), src),
                    M::Por => E::bin(B::Or, W::B64, dst, src),
                    _ => E::bin(B::Xor, W::B64, dst, src),
                };
                self.stmts.push(S::SetMm(d, v));
            }
            // movd mm, r/m32 (zero-extended); movq mm, mm/m64.
            M::Movd | M::Movq if d_mm => {
                let d = Self::mm_dst(ins)?;
                let src = self.mm_src(ins, 1)?;
                self.stmts.push(S::SetMm(d, src));
            }
            // movd r/m32, mm (the low half); movq mm/m64, mm.
            _ => {
                let n = mm(ins.op1_register()).ok_or("MMX source")?;
                let l = self.loc(ins, 0)?;
                let v = match &l {
                    Loc::Mem(w, _) => E::Mm(n).trunc(*w),
                    _ => E::Mm(n).trunc(W::B32),
                };
                self.set(&l, v)?;
            }
        }
        Ok(true)
    }
}

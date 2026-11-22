// SPDX-License-Identifier: MIT
//! IR to Rust source, the developer back end.
//!
//! The sister projects emit C (`rc-emit-c`) and compile it with a C compiler. This project uses
//! no C: every guest function becomes a Rust function with the same contract as the other back
//! ends (`rc-emit-native`, `rc-emit-wasm`):
//!
//! ```text
//! pub fn technomage_exe_cd_de_00401000(c: &mut Cpu) { ... }
//! ```
//!
//! Registers and the six arithmetic flags are locals, written back to `c` around every call,
//! return and indirect transfer (the flags at returns and after calls with
//! [`Options::flags_at_return`], the default). Guest memory is reached through `m`, a copy of the
//! runtime's [`Mem`] handle (`rc_runtime::lifted::Mem`). Direct calls are Rust calls, tail
//! transfers to other functions are `return f(c)`, indirect ones go through `dispatch`.
//!
//! Rust has no `goto`: a function is cut into blocks at its labels (`Function::labels`, which
//! hold the entry, every jump target and every fall-through after a branch) and the blocks are
//! the arms of a `match` inside a `loop`, the same structure `rc-emit-wasm` uses. A jump sets the
//! block and continues the loop; falling into the next block does the same. A function entered
//! above its lowest instruction simply starts in the entry's block.
//!
//! Output layout (`generated/<game>/<binary>-<variant>/`, git-ignored): `code_XXXX.rs` per 64 KiB
//! of guest address space (by the function's middle instruction) and `mod.rs`, which declares them, re-exports every function and holds
//! the sorted dispatch table `TABLE`. The output is a function of the input only.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use rc_ir::{
    BinOp as B, Expr as E, FArith, FBinary, FConst, FLoc, FUnary, Flag, Fpu, Function, Helper,
    Part, Reg, Stmt as S, Term, UnOp as U, W,
};

const REGS: [&str; 8] = ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"];
const FLAGS: [&str; 6] = ["cf", "pf", "af", "zf", "sf", "of"];

/// How the emitter names and finds functions.
pub trait Program {
    /// The Rust name of the function at `addr`.
    fn name(&self, addr: u32) -> String;
    /// Whether `addr` is a function entry.
    fn is_entry(&self, addr: u32) -> bool;
}

/// Names functions `<prefix>xxxxxxxx`.
#[derive(Debug, Clone)]
pub struct Entries {
    pub prefix: String,
    pub entries: BTreeSet<u32>,
}

impl Program for Entries {
    fn name(&self, addr: u32) -> String {
        format!("{}{addr:08x}", self.prefix)
    }
    fn is_entry(&self, addr: u32) -> bool {
        self.entries.contains(&addr)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Put each guest instruction as a comment above its Rust.
    pub comments: bool,
    /// The flags cross function boundaries: stored into `c` at every `ret` and read back after
    /// every call. Hand-written library code returns results in them (the CRT's math helpers).
    /// On by default.
    pub flags_at_return: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            comments: false,
            flags_at_return: true,
        }
    }
}

fn rtype(w: W) -> &'static str {
    match w {
        W::B8 => "u8",
        W::B16 => "u16",
        W::B32 => "u32",
        W::B64 => "u64",
    }
}

fn stype(w: W) -> &'static str {
    match w {
        W::B8 => "i8",
        W::B16 => "i16",
        W::B32 => "i32",
        W::B64 => "i64",
    }
}

fn flag(f: Flag) -> &'static str {
    match f {
        Flag::Cf => "cf",
        Flag::Pf => "pf",
        Flag::Af => "af",
        Flag::Zf => "zf",
        Flag::Sf => "sf",
        Flag::Of => "of",
        Flag::Df => "c.df",
    }
}

fn reg(r: Reg) -> String {
    let n = REGS[usize::from(r.n)];
    match r.part {
        Part::D => n.into(),
        Part::X => format!("({n} as u16)"),
        Part::L => format!("({n} as u8)"),
        Part::H => format!("(({n} >> 8) as u8)"),
    }
}

/// The Rust type of a rendered expression: exactly the unsigned type of a width, or `bool`
/// (comparisons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    U(W),
    Bool,
}

/// Render an expression with its Rust type.
fn ex(e: &E) -> (String, Ty) {
    match e {
        E::Const(w, v) => (format!("{v:#x}{}", rtype(*w)), Ty::U(*w)),
        E::Tmp(w, t) => (format!("t{t}"), Ty::U(*w)),
        E::Reg(r) => (reg(*r), Ty::U(r.part.width())),
        E::Flag(f) => (flag(*f).into(), Ty::U(W::B8)),
        E::FsBase => ("c.fs_base".into(), Ty::U(W::B32)),
        E::FpuStatus => ("fstsw(c)".into(), Ty::U(W::B16)),
        E::FpuControl => ("c.fpu_control".into(), Ty::U(W::B16)),
        E::Load(w, a) => (format!("m.ld{}({})", w.bits(), exact(a, W::B32)), Ty::U(*w)),
        E::Bin(op, w, a, b) => bin(*op, *w, a, b),
        E::Un(op, w, a) => {
            let x = exact(a, *w);
            match op {
                U::Not => (format!("(!{x})"), Ty::U(*w)),
                U::Neg => (format!("{x}.wrapping_neg()"), Ty::U(*w)),
                U::Parity => (format!("parity8({})", exact(a, W::B8)), Ty::U(W::B8)),
                U::Ctz => (format!("ctz{}({x})", w.bits()), Ty::U(*w)),
                U::Clz => (format!("clz{}({x})", w.bits()), Ty::U(*w)),
                U::Bswap => (format!("{}.swap_bytes()", exact(a, W::B32)), Ty::U(W::B32)),
            }
        }
        E::Zext(to, a) | E::Trunc(to, a) => (exact(a, *to), Ty::U(*to)),
        E::Sext(from, to, a) => (
            format!(
                "(({} as {}) as {} as {})",
                exact(a, *from),
                stype(*from),
                stype(*to),
                rtype(*to)
            ),
            Ty::U(*to),
        ),
        E::Select(w, c, a, b) => (
            format!(
                "(if {} {{ {} }} else {{ {} }})",
                cond(c),
                exact(a, *w),
                exact(b, *w)
            ),
            Ty::U(*w),
        ),
        E::Mm(n) => (format!("c.mm[{n}]"), Ty::U(W::B64)),
        E::Mmx(op, a, b) => (
            format!(
                "mmx_{}({}, {})",
                op.name(),
                exact(a, W::B64),
                exact(b, W::B64)
            ),
            Ty::U(W::B64),
        ),
    }
}

/// `e` with exactly the Rust type of `w`.
fn exact(e: &E, w: W) -> String {
    let (s, t) = ex(e);
    match t {
        Ty::U(x) if x == w => s,
        Ty::U(_) => format!("({s} as {})", rtype(w)),
        Ty::Bool => format!("({s} as {})", rtype(w)),
    }
}

/// `e` as a condition (non-zero is true).
fn cond(e: &E) -> String {
    match ex(e) {
        (s, Ty::Bool) => s,
        (s, Ty::U(_)) => format!("({s} != 0)"),
    }
}

fn bin(op: B, w: W, a: &E, b: &E) -> (String, Ty) {
    let (t, s) = (rtype(w), stype(w));
    let u = Ty::U(w);
    let (x, y) = (exact(a, w), exact(b, w));
    let method = |m: &str| (format!("{x}.{m}({y})"), u);
    let constant = match b {
        E::Const(_, n) if *n < u64::from(w.bits()) => Some(*n),
        _ => None,
    };
    match op {
        B::Add => method("wrapping_add"),
        B::Sub => method("wrapping_sub"),
        B::Mul => method("wrapping_mul"),
        B::And => (format!("({x} & {y})"), u),
        B::Or => (format!("({x} | {y})"), u),
        B::Xor => (format!("({x} ^ {y})"), u),
        // A zero divisor is a guest #DE; the lifter guards division, and a panic names it.
        B::UDiv => (format!("({x} / {y})"), u),
        B::URem => (format!("({x} % {y})"), u),
        B::SDiv => (format!("(({x} as {s}).wrapping_div({y} as {s}) as {t})"), u),
        B::SRem => (format!("(({x} as {s}).wrapping_rem({y} as {s}) as {t})"), u),
        B::Shl | B::Shr | B::Sar if constant == Some(0) => (x, u),
        B::Shl if constant.is_some() => (format!("({x} << {})", constant.unwrap_or(0)), u),
        B::Shr if constant.is_some() => (format!("({x} >> {})", constant.unwrap_or(0)), u),
        B::Sar if constant.is_some() => (
            format!("((({x} as {s}) >> {}) as {t})", constant.unwrap_or(0)),
            u,
        ),
        B::Shl | B::Shr | B::Sar => {
            let name = match op {
                B::Shl => "shl",
                B::Shr => "shr",
                _ => "sar",
            };
            (format!("{name}{}({x}, {})", w.bits(), exact(b, W::B32)), u)
        }
        B::Eq | B::Ne | B::Ult | B::Ule => {
            let o = match op {
                B::Eq => "==",
                B::Ne => "!=",
                B::Ult => "<",
                _ => "<=",
            };
            (format!("({x} {o} {y})"), Ty::Bool)
        }
        B::Slt | B::Sle => {
            let o = if op == B::Slt { "<" } else { "<=" };
            (format!("(({x} as {s}) {o} ({y} as {s}))"), Ty::Bool)
        }
    }
}

/// An x87 operand's value as an `f64`.
fn fsrc(l: &FLoc) -> String {
    let a = |e: &E| exact(e, W::B32);
    match l {
        FLoc::St(i) => format!("st(c, {i})"),
        FLoc::F32(e) => format!("f32_of(m.ld32({}))", a(e)),
        FLoc::F64(e) => format!("f64_of(m.ld64({}))", a(e)),
        FLoc::F80(e) => format!("f80(m, {})", a(e)),
        FLoc::I16(e) => format!("(m.ld16({}) as i16 as f64)", a(e)),
        FLoc::I32(e) => format!("(m.ld32({}) as i32 as f64)", a(e)),
        FLoc::I64(e) => format!("(m.ld64({}) as i64 as f64)", a(e)),
    }
}

/// Rust statements for an x87 operation.
fn fpu(f: &Fpu) -> String {
    let pop = |p: bool| if p { " fpop(c);" } else { "" };
    let a = |e: &E| exact(e, W::B32);
    match f {
        Fpu::Load(l) => format!("{{ let v = {}; fpush(c, v); }}", fsrc(l)),
        Fpu::Const(k) => {
            let n = match k {
                FConst::One => 0,
                FConst::Zero => 1,
                FConst::Pi => 2,
                FConst::L2e => 3,
                FConst::L2t => 4,
                FConst::Lg2 => 5,
                FConst::Ln2 => 6,
            };
            format!("fconst(c, {n});")
        }
        Fpu::Store { dst, pop: p } => {
            let st = match dst {
                FLoc::St(i) => format!("{{ let v = st(c, 0); set_st(c, {i}, v); }}"),
                FLoc::F32(e) => format!("m.st32({}, f32_bits(st(c, 0)));", a(e)),
                FLoc::F64(e) => format!("m.st64({}, st(c, 0).to_bits());", a(e)),
                FLoc::F80(e) => format!("st_f80(m, {}, st(c, 0));", a(e)),
                FLoc::I16(e) => format!("m.st16({}, fist(c, 16) as u16);", a(e)),
                FLoc::I32(e) => format!("m.st32({}, fist(c, 32) as u32);", a(e)),
                FLoc::I64(e) => format!("m.st64({}, fist(c, 64));", a(e)),
            };
            format!("{st}{}", pop(*p))
        }
        Fpu::Arith {
            op,
            dst,
            src,
            pop: p,
        } => {
            let n = match op {
                FArith::Add => 0,
                FArith::Mul => 1,
                FArith::Sub => 2,
                FArith::SubR => 3,
                FArith::Div => 4,
                FArith::DivR => 5,
            };
            format!(
                "{{ let v = {}; farith(c, {n}, {dst}, v); }}{}",
                fsrc(src),
                pop(*p)
            )
        }
        Fpu::Unary(u) => {
            let n = match u {
                FUnary::Chs => 0,
                FUnary::Abs => 1,
                FUnary::Sqrt => 2,
                FUnary::Sin => 3,
                FUnary::Cos => 4,
                FUnary::Rndint => 5,
                FUnary::F2xm1 => 6,
                FUnary::Tst => 7,
                FUnary::Xam => 8,
            };
            format!("funary(c, {n});")
        }
        Fpu::Binary(b) => {
            let n = match b {
                FBinary::Patan => 0,
                FBinary::Yl2x => 1,
                FBinary::Scale => 2,
                FBinary::Prem => 3,
                FBinary::Prem1 => 4,
                FBinary::Ptan => 5,
            };
            format!("fbinary(c, {n});")
        }
        Fpu::Compare { src, pops } => {
            let mut s = format!("{{ let v = {}; fcom(c, v); }}", fsrc(src));
            for _ in 0..*pops {
                s.push_str(" fpop(c);");
            }
            s
        }
        Fpu::Xch(i) => format!("fxch(c, {i});"),
        Fpu::Free(i) => format!("ffree(c, {i});"),
        Fpu::IncStp => "c.top = c.top.wrapping_add(1) & 7;".into(),
        Fpu::DecStp => "c.top = c.top.wrapping_sub(1) & 7;".into(),
        Fpu::Init => "finit(c);".into(),
        Fpu::Clex => "c.fpu_status &= 0x7f00;".into(),
        Fpu::Ldcw(v) => format!("c.fpu_control = {};", exact(v, W::B16)),
        Fpu::Stenv(e) => format!("fstenv(c, m, {});", a(e)),
        Fpu::Ldenv(e) => format!("fldenv(c, m, {});", a(e)),
        Fpu::Save(e) => format!("fsave(c, m, {});", a(e)),
        Fpu::Rstor(e) => format!("frstor(c, m, {});", a(e)),
    }
}

fn spill_regs() -> String {
    let mut s = String::new();
    for (i, r) in REGS.iter().enumerate() {
        let _ = write!(s, "c.r[{i}] = {r}; ");
    }
    s
}

fn spill_flags() -> String {
    let mut s = String::new();
    for n in FLAGS {
        let _ = write!(s, "c.{n} = {n}; ");
    }
    s
}

fn spill_all() -> String {
    spill_regs() + &spill_flags()
}

fn reload_regs() -> String {
    let mut s = String::new();
    for (i, r) in REGS.iter().enumerate() {
        let _ = write!(s, "{r} = c.r[{i}]; ");
    }
    s
}

fn reload_all() -> String {
    let mut s = reload_regs();
    for n in FLAGS {
        let _ = write!(s, "{n} = c.{n}; ");
    }
    s
}

/// A Rust string literal (printable ASCII; anything else becomes `?`).
fn rstr(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' | '\\' => {
                o.push('\\');
                o.push(ch);
            }
            c if c.is_ascii_graphic() || c == ' ' => o.push(c),
            _ => o.push('?'),
        }
    }
    o.push('"');
    o
}

struct Emitter<'a> {
    p: &'a dyn Program,
    opts: Options,
    f: &'a Function,
    /// Block number of every block start (`Function::labels` and the jump targets in the
    /// function), in address order.
    blocks: BTreeMap<u32, usize>,
    own: BTreeSet<u32>,
    out: String,
    depth: usize,
    unwind_range: (u32, u32),
    continuations: Vec<u32>,
    /// Flag settings nothing reads, by instruction and statement (`rc_emit_wasm::flags`).
    dead: Vec<Vec<bool>>,
}

impl Emitter<'_> {
    fn line(&mut self, s: &str) {
        for _ in 0..self.depth {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// After a call: while an exception unwinds, return (after letting this frame take the
    /// continuation if it is the one: same ebp, a place in this function's code).
    fn unwind_check(&self) -> String {
        let (lo, hi) = self.unwind_range;
        let at = if self.continuations.is_empty() {
            "&[]"
        } else {
            "UNWIND_AT"
        };
        format!("if c.unwind != 0 {{ unwind_claim(c, ebp, {lo:#x}, {hi:#x}, {at}); return; }} ")
    }

    fn reload_after_call(&self) -> String {
        if self.opts.flags_at_return {
            reload_all()
        } else {
            reload_regs()
        }
    }

    /// Control goes to `t`: a block of this function, a tail call, or a trap.
    fn transfer(&mut self, t: u32, from: u32) {
        if let Some(b) = self.blocks.get(&t) {
            let l = format!("b = {b}; continue 'f;");
            self.line(&l);
        } else if self.own.contains(&t) {
            // Every jump target is a label; reaching here means the IR is inconsistent.
            let l = format!(
                "{}trap(c, {from:#010x}, \"transfer to {t:#010x}, inside a block\");",
                spill_all()
            );
            self.line(&l);
        } else if self.p.is_entry(t) {
            let l = format!("{}return {}(c);", spill_all(), self.p.name(t));
            self.line(&l);
        } else {
            let l = format!(
                "{}trap(c, {from:#010x}, \"transfer to {t:#010x}, not a known entry\");",
                spill_all()
            );
            self.line(&l);
        }
    }

    fn stmts(&mut self, body: &[S], addr: u32) {
        for s in body {
            self.stmt(s, addr);
        }
    }

    fn stmt(&mut self, s: &S, addr: u32) {
        match s {
            S::Let(t, e) => {
                let w = e.width();
                let l = format!("let t{t}: {} = {};", rtype(w), exact(e, w));
                self.line(&l);
            }
            S::SetReg(r, e) => {
                let n = REGS[usize::from(r.n)];
                let l = match r.part {
                    Part::D => format!("{n} = {};", exact(e, W::B32)),
                    Part::X => format!("{n} = ({n} & 0xffff0000) | ({} as u32);", exact(e, W::B16)),
                    Part::L => format!("{n} = ({n} & 0xffffff00) | ({} as u32);", exact(e, W::B8)),
                    Part::H => format!(
                        "{n} = ({n} & 0xffff00ff) | (({} as u32) << 8);",
                        exact(e, W::B8)
                    ),
                };
                self.line(&l);
            }
            S::SetFlag(f, e) => {
                let l = format!("{} = {};", flag(*f), exact(e, W::B8));
                self.line(&l);
            }
            S::Store(a, v) => {
                let w = v.width();
                let l = format!("m.st{}({}, {});", w.bits(), exact(a, W::B32), exact(v, w));
                self.line(&l);
            }
            S::If(c, body) => {
                let l = format!("if {} {{", cond(c));
                self.line(&l);
                self.depth += 1;
                self.stmts(body, addr);
                self.depth -= 1;
                self.line("}");
            }
            S::Loop(body) => {
                self.line("loop {");
                self.depth += 1;
                self.stmts(body, addr);
                self.depth -= 1;
                self.line("}");
            }
            S::BreakIf(c) => {
                let l = format!("if {} {{ break; }}", cond(c));
                self.line(&l);
            }
            S::Helper(Helper::Cpuid) => {
                self.line("cpuid(&mut eax, &mut ebx, &mut ecx, &mut edx);");
            }
            S::Helper(Helper::Rdtsc) => self.line("rdtsc(&mut eax, &mut edx);"),
            S::Fpu(f) => self.line(&fpu(f)),
            S::SetMm(n, e) => {
                let l = format!("c.mm[{n}] = {};", exact(e, W::B64));
                self.line(&l);
            }
            S::Point(at) => {
                let l = format!("{}point(c, {at:#010x}); {}", spill_all(), reload_all());
                self.line(&l);
            }
            S::Trap(why) => {
                let l = format!("{}trap(c, {addr:#010x}, {});", spill_all(), rstr(why));
                self.line(&l);
            }
        }
    }

    fn function(&mut self) {
        let f = self.f;
        let name = self.p.name(f.addr);
        let _ = writeln!(self.out, "pub fn {name}(c: &mut Cpu) {{");
        self.depth = 1;
        self.line("let m = c.mem();");
        let regs: Vec<String> = REGS
            .iter()
            .enumerate()
            .map(|(i, r)| format!("let mut {r}: u32 = c.r[{i}];"))
            .collect();
        self.line(&regs.join(" "));
        let flags: Vec<String> = FLAGS
            .iter()
            .map(|n| format!("let mut {n}: u8 = c.{n};"))
            .collect();
        self.line(&flags.join(" "));
        if !self.continuations.is_empty() {
            let list: Vec<String> = self
                .continuations
                .iter()
                .map(|a| format!("{a:#x}"))
                .collect();
            let l = format!("const UNWIND_AT: &[u32] = &[{}];", list.join(", "));
            self.line(&l);
        }
        // An __except block runs in its function's frame: called by __except_handler3 during an
        // exception, it first unwinds the lifted functions to that frame, which then enters it
        // again (unwind_claim sets resume_eip).
        if f.except_block {
            let l = format!(
                "if c.resume_eip == {a:#x} {{ c.resume_eip = 0; }} else if c.eh_depth != 0 {{ unwind_begin(c, {a:#x}); return; }}",
                a = f.addr
            );
            self.line(&l);
        }
        let entry = self.blocks.get(&f.addr).copied().unwrap_or(0);
        let l = format!("let mut b: usize = {entry};");
        self.line(&l);
        self.line("'f: loop {");
        self.depth += 1;
        self.line("match b {");
        self.depth += 1;

        let mut open = false;
        for (i, ins) in f.insns.iter().enumerate() {
            if let Some(&bn) = self.blocks.get(&ins.addr) {
                if open {
                    // Falling into this block from the one before.
                    let l = format!("b = {bn}; continue 'f;");
                    self.line(&l);
                    self.depth -= 1;
                    self.line("}");
                }
                let l = format!("{bn} => {{");
                self.line(&l);
                self.depth += 1;
                open = true;
            } else if !open {
                // An instruction nothing reaches by falling through or jumping (after a
                // terminator, not a label): unreachable; skip it.
                continue;
            }
            if self.opts.comments && !ins.text.is_empty() {
                let c = format!("// {:08x}: {}", ins.addr, ins.text);
                self.line(&c);
            }
            self.line("{");
            self.depth += 1;
            let dead = std::mem::take(&mut self.dead[i]);
            for (k, s) in ins.stmts.iter().enumerate() {
                if !(matches!(s, S::SetFlag(..)) && dead.get(k).copied().unwrap_or(false)) {
                    self.stmt(s, ins.addr);
                }
            }
            let next = ins.next();
            let next_emitted = f.insns.get(i + 1).map(|n| n.addr);
            // Fall through: either the next instruction continues this block, or a transfer.
            let falls_in_block = next_emitted == Some(next) && !self.blocks.contains_key(&next);
            let mut ends = true;
            match &ins.term {
                Term::Next => {
                    if falls_in_block {
                        ends = false;
                    } else {
                        self.transfer(next, ins.addr);
                    }
                }
                Term::Jump(t) => self.transfer(*t, ins.addr),
                Term::Branch(c, t) => {
                    let l = format!("if {} {{", cond(c));
                    self.line(&l);
                    self.depth += 1;
                    self.transfer(*t, ins.addr);
                    self.depth -= 1;
                    self.line("}");
                    if falls_in_block {
                        ends = false;
                    } else {
                        self.transfer(next, ins.addr);
                    }
                }
                Term::Call { target, ret } => {
                    if self.p.is_entry(*target) {
                        let l = format!(
                            "esp = esp.wrapping_sub(4); m.st32(esp, {ret:#x}); {}{}(c); {}{}",
                            spill_regs(),
                            self.p.name(*target),
                            self.unwind_check(),
                            self.reload_after_call()
                        );
                        self.line(&l);
                        if falls_in_block {
                            ends = false;
                        } else {
                            self.transfer(next, ins.addr);
                        }
                    } else {
                        let l = format!(
                            "{}trap(c, {:#010x}, \"call to {target:#010x}, not a known entry\");",
                            spill_all(),
                            ins.addr
                        );
                        self.line(&l);
                    }
                }
                Term::CallIndirect { target, ret } => {
                    let l = format!(
                        "let t_: u32 = {}; esp = esp.wrapping_sub(4); m.st32(esp, {ret:#x}); {}dispatch(c, t_); {}{}",
                        exact(target, W::B32),
                        spill_regs(),
                        self.unwind_check(),
                        self.reload_after_call()
                    );
                    self.line(&l);
                    if falls_in_block {
                        ends = false;
                    } else {
                        self.transfer(next, ins.addr);
                    }
                }
                Term::Ret(n) => {
                    let spill = if self.opts.flags_at_return {
                        spill_all()
                    } else {
                        spill_regs()
                    };
                    let l = format!(
                        "esp = esp.wrapping_add({}); {spill}return;",
                        4 + u32::from(*n)
                    );
                    self.line(&l);
                }
                Term::JumpIndirect(t) => {
                    let l = format!(
                        "let t_: u32 = {}; {}dispatch_jump(c, t_); return;",
                        exact(t, W::B32),
                        spill_all()
                    );
                    self.line(&l);
                }
                Term::JumpTable { target, cases } => {
                    let l = format!("match {} {{", exact(target, W::B32));
                    self.line(&l);
                    self.depth += 1;
                    let mut seen = BTreeSet::new();
                    for &t in cases {
                        if seen.insert(t) {
                            let l = format!("{t:#010x} => {{");
                            self.line(&l);
                            self.depth += 1;
                            self.transfer(t, ins.addr);
                            self.depth -= 1;
                            self.line("}");
                        }
                    }
                    let l = format!(
                        "_ => {{ {}trap(c, {:#010x}, \"jump table target outside the table\"); }}",
                        spill_all(),
                        ins.addr
                    );
                    self.line(&l);
                    self.depth -= 1;
                    self.line("}");
                }
                Term::Stop => {
                    let l = format!(
                        "{}trap(c, {:#010x}, \"execution stopped\");",
                        spill_all(),
                        ins.addr
                    );
                    self.line(&l);
                }
            }
            self.depth -= 1;
            self.line("}");
            if ends {
                self.depth -= 1;
                self.line("}");
                open = false;
            }
        }
        if open {
            // The last instruction fell off the end: cannot happen for valid IR.
            self.line(&format!(
                "{}trap(c, {:#010x}, \"fell off the end of the function\");",
                spill_all(),
                f.addr
            ));
            self.depth -= 1;
            self.line("}");
        }
        let l = format!(
            "_ => {{ {}trap(c, {:#010x}, \"no such block\"); }}",
            spill_all(),
            f.addr
        );
        self.line(&l);
        self.depth -= 1;
        self.line("}");
        self.depth -= 1;
        self.line("}");
        self.out.push_str("}\n");
    }
}

/// The instruction span of `f` (where an exception may continue in its frame).
fn unwind_range(f: &Function) -> (u32, u32) {
    let lo = f.insns.iter().map(|i| i.addr).min().unwrap_or(f.addr);
    let hi = f
        .insns
        .iter()
        .map(|i| i.addr + u32::from(i.len))
        .max()
        .unwrap_or(f.addr);
    (lo, hi)
}

/// Block starts of `f`: its labels, its entry, and every target of a transfer inside it.
fn block_starts(f: &Function) -> BTreeMap<u32, usize> {
    let own: BTreeSet<u32> = f.insns.iter().map(|i| i.addr).collect();
    let mut starts: BTreeSet<u32> = f
        .labels
        .iter()
        .copied()
        .filter(|a| own.contains(a))
        .collect();
    if own.contains(&f.addr) {
        starts.insert(f.addr);
    }
    if let Some(first) = f.insns.first() {
        starts.insert(first.addr);
    }
    for (i, ins) in f.insns.iter().enumerate() {
        let next = ins.next();
        let next_emitted = f.insns.get(i + 1).map(|n| n.addr);
        let mut add = |t: u32| {
            if own.contains(&t) {
                starts.insert(t);
            }
        };
        match &ins.term {
            Term::Jump(t) => add(*t),
            Term::Branch(_, t) => {
                add(*t);
                add(next);
            }
            Term::JumpTable { cases, .. } => cases.iter().for_each(|&t| add(t)),
            Term::Next | Term::Call { .. } | Term::CallIndirect { .. }
                if next_emitted != Some(next) =>
            {
                add(next);
            }
            _ => {}
        }
        // After a terminator, the next instruction can only be reached as a block.
        if matches!(
            ins.term,
            Term::Jump(_)
                | Term::Ret(_)
                | Term::JumpIndirect(_)
                | Term::JumpTable { .. }
                | Term::Stop
        ) {
            if let Some(n) = next_emitted {
                starts.insert(n);
            }
        }
    }
    starts
        .into_iter()
        .enumerate()
        .map(|(i, a)| (a, i))
        .collect()
}

/// The Rust definition of one function.
pub fn function(p: &dyn Program, f: &Function, opts: Options) -> String {
    let mut e = Emitter {
        p,
        opts,
        f,
        blocks: block_starts(f),
        own: f.insns.iter().map(|i| i.addr).collect(),
        out: String::new(),
        depth: 0,
        unwind_range: unwind_range(f),
        continuations: f.continuations.clone(),
        dead: rc_emit_wasm::flags::dead_settings(f, |a| p.is_entry(a), opts.flags_at_return),
    };
    e.function();
    e.out
}

/// A generated source tree: file name → contents.
pub type Tree = BTreeMap<String, String>;

const HEADER: &str = "// Generated by recomp from the user's own copy of the game.\n// Derived from the game's executable: never commit or distribute.\n";

const ALLOW: &str = "#![allow(unused_mut, unused_variables, unused_assignments, unreachable_code, unused_labels, unused_parens, unused_braces, non_snake_case, clippy::all)]\n";

/// One binary of a program that links several: one file per 64 KiB of guest code and `mod.rs`
/// with the module declarations, the re-exports and `TABLE`, the sorted `(address, function)`
/// pairs the dispatcher searches. `functions` must be sorted by address.
pub fn binary(p: &Entries, functions: &[Function], opts: Options) -> Tree {
    let mut tree = Tree::new();
    let mut buckets: BTreeMap<u32, String> = BTreeMap::new();
    for f in functions {
        // By where the body is, not the entry: an entry is often a jump thunk far from it, and
        // its lowest instruction may be that thunk too. The middle instruction is in the body.
        let middle = f.insns.get(f.insns.len() / 2).map_or(f.addr, |i| i.addr);
        let b = buckets.entry(middle >> 16).or_default();
        b.push_str(&function(p, f, opts));
        b.push('\n');
    }
    let mut module = format!("{HEADER}{ALLOW}\nuse rc_runtime::lifted::Cpu;\n\n");
    for (k, body) in buckets {
        let _ = writeln!(module, "mod code_{k:04x};\npub use code_{k:04x}::*;");
        let src = format!("{HEADER}{ALLOW}\nuse super::*;\nuse rc_runtime::lifted::*;\n\n{body}");
        tree.insert(format!("code_{k:04x}.rs"), src);
    }
    let _ = writeln!(
        module,
        "\n/// Every lifted function of this binary, sorted by guest address.\npub static TABLE: [(u32, fn(&mut Cpu)); {}] = [",
        functions.len()
    );
    for f in functions {
        let _ = writeln!(module, "    ({:#010x}, {}),", f.addr, p.name(f.addr));
    }
    module.push_str("];\n");
    tree.insert("mod.rs".into(), module);
    tree
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_ir::Insn;

    fn insn(addr: u32, stmts: Vec<S>, term: Term) -> Insn {
        Insn {
            addr,
            len: 1,
            text: String::new(),
            stmts,
            term,
        }
    }

    fn names(entries: &[u32]) -> Entries {
        Entries {
            prefix: "f_".into(),
            entries: entries.iter().copied().collect(),
        }
    }

    #[test]
    fn a_function_entered_above_its_lowest_instruction_starts_in_its_block() {
        // 0x1000: ret; 0x1001 (the entry): jmp 0x1000.
        let f = Function {
            addr: 0x1001,
            insns: vec![
                insn(0x1000, vec![], Term::Ret(0)),
                insn(0x1001, vec![], Term::Jump(0x1000)),
            ],
            labels: vec![0x1000, 0x1001],
            ..Default::default()
        };
        let r = function(&names(&[0x1001]), &f, Options::default());
        assert!(r.contains("let mut b: usize = 1;"), "{r}");
        assert!(r.contains("b = 0; continue 'f;"), "{r}");
    }

    #[test]
    fn comparisons_are_bools_and_widths_are_exact() {
        let t = |w, n| E::Tmp(w, n);
        assert_eq!(
            exact(&E::bin(B::Add, W::B32, t(W::B32, 0), E::c32(4)), W::B32),
            "t0.wrapping_add(0x4u32)"
        );
        assert_eq!(
            exact(&E::bin(B::Eq, W::B8, t(W::B8, 0), t(W::B8, 1)), W::B8),
            "((t0 == t1) as u8)"
        );
        assert_eq!(
            exact(&E::bin(B::Shr, W::B32, t(W::B32, 0), t(W::B8, 1)), W::B32),
            "shr32(t0, (t1 as u32))"
        );
        assert_eq!(
            exact(
                &E::bin(B::Sar, W::B16, t(W::B16, 0), E::c(W::B16, 3)),
                W::B16
            ),
            "(((t0 as i16) >> 3) as u16)"
        );
    }

    #[test]
    fn calls_spill_and_reload() {
        let f = Function {
            addr: 0x1000,
            insns: vec![
                insn(
                    0x1000,
                    vec![],
                    Term::Call {
                        target: 0x2000,
                        ret: 0x1001,
                    },
                ),
                insn(0x1001, vec![], Term::Ret(4)),
            ],
            labels: vec![0x1000],
            ..Default::default()
        };
        let r = function(&names(&[0x1000, 0x2000]), &f, Options::default());
        assert!(r.contains("f_00002000(c);"), "{r}");
        assert!(r.contains("esp = esp.wrapping_add(8);"), "{r}");
        // The call falls through within the block: no second block.
        assert!(!r.contains("1 => {"), "{r}");
    }

    #[test]
    fn binary_tree_has_a_table() {
        let f = Function {
            addr: 0x401000,
            insns: vec![insn(0x401000, vec![], Term::Ret(0))],
            labels: vec![0x401000],
            ..Default::default()
        };
        let t = binary(&names(&[0x401000]), &[f], Options::default());
        assert!(t["mod.rs"].contains("(0x00401000, f_00401000),"));
        assert!(t.contains_key("code_0040.rs"));
    }
}

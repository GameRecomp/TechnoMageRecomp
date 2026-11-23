// SPDX-License-Identifier: MIT
//! Lifted code as native machine code from the converter (`rc-emit-native`, Cranelift): a blob
//! with the functions, their table, trap messages and module names, which is copied into
//! executable memory and entered through the dispatcher. The code reaches the runtime through
//! `Cpu::helpers`, a table of C-ABI functions in the order `rc-emit-native`'s `helper` module
//! numbers them. As the sister projects' `native.c` (blob format 2).

use crate::cpu::{Cpu, EAX, EBX, ECX, EDX};
use crate::dispatch;
use crate::mem::Mem;
use crate::x87;
use std::sync::Mutex;

/// A function of native code: the platform's C calling convention, the CPU state's address.
pub type NativeFn = unsafe extern "C" fn(*mut Cpu);

/// The target this runtime runs native code for, as the blob numbers them.
pub fn host_target() -> u32 {
    if cfg!(all(target_arch = "x86_64", windows)) {
        2
    } else if cfg!(target_arch = "x86_64") {
        1
    } else if cfg!(all(target_arch = "aarch64", target_vendor = "apple")) {
        3
    } else if cfg!(target_arch = "aarch64") {
        4
    } else {
        0
    }
}

/// The target's name in package file names (`data/code-<target>-<exe>.bin`).
pub fn target_name() -> Option<&'static str> {
    match host_target() {
        1 => Some("x86_64-sysv"),
        2 => Some("x86_64-windows"),
        3 => Some("arm64-apple"),
        4 => Some("arm64-linux"),
        _ => None,
    }
}

static TRAPS: Mutex<Vec<(u32, u32, String)>> = Mutex::new(Vec::new());

/// The CPU state behind the pointer native code passes to a helper.
fn cpu<'a>(c: *mut Cpu) -> &'a mut Cpu {
    // SAFETY: native code calls the helpers with the `Cpu` it was given, which is valid and not
    // otherwise borrowed while the call runs.
    unsafe { &mut *c }
}
unsafe extern "C" fn h_dispatch(c: *mut Cpu, target: u32) {
    let c = cpu(c);
    dispatch::dispatch(c, target);
}

unsafe extern "C" fn h_trap(c: *mut Cpu, addr: u32, kind: u32, arg: u32) {
    let c = cpu(c);
    let why = match kind {
        0 => TRAPS
            .lock()
            .expect("traps")
            .iter()
            .find(|t| t.0 == addr && t.1 == arg)
            .map_or_else(|| "a trap".to_string(), |t| t.2.clone()),
        1 => format!("transfer to {arg:#010x}, not a known entry"),
        2 => format!("call to {arg:#010x}, not a known entry"),
        _ => "jump table target outside the table".to_string(),
    };
    dispatch::trap(c, addr, &why);
}

unsafe extern "C" fn h_cpuid(c: *mut Cpu) {
    let c = cpu(c);
    let (mut a, mut b, mut cx, mut d) = (c.r[EAX], c.r[EBX], c.r[ECX], c.r[EDX]);
    dispatch::cpuid(&mut a, &mut b, &mut cx, &mut d);
    c.r[EAX] = a;
    c.r[EBX] = b;
    c.r[ECX] = cx;
    c.r[EDX] = d;
}

unsafe extern "C" fn h_rdtsc(c: *mut Cpu) {
    let c = cpu(c);
    let (mut a, mut d) = (0, 0);
    dispatch::rdtsc(&mut a, &mut d);
    c.r[EAX] = a;
    c.r[EDX] = d;
}

fn fsrc(c: &Cpu, m: Mem, code: u32, a: u32) -> f64 {
    match (code >> 8) & 15 {
        0 => x87::st(c, ((code >> 12) & 7) as u8),
        1 => x87::f32_of(m.ld32(a)),
        2 => x87::f64_of(m.ld64(a)),
        3 => x87::f80(m, a),
        4 => f64::from(m.ld16(a) as i16),
        5 => f64::from(m.ld32(a) as i32),
        _ => m.ld64(a) as i64 as f64,
    }
}

unsafe extern "C" fn h_fpu(c: *mut Cpu, code: u32, a: u32) {
    let c = cpu(c);
    let m = c.mem();
    let sub = (code >> 16) & 15;
    let dst = ((code >> 20) & 7) as u8;
    let pop = (code >> 24) & 3;
    let st = ((code >> 12) & 7) as u8;
    match code & 0xff {
        0 => {
            let v = fsrc(c, m, code, a);
            x87::fpush(c, v);
        }
        1 => x87::fconst(c, sub),
        2 => {
            match (code >> 8) & 15 {
                0 => {
                    let v = x87::st(c, 0);
                    x87::set_st(c, st, v);
                }
                1 => m.st32(a, x87::f32_bits(x87::st(c, 0))),
                2 => m.st64(a, x87::st(c, 0).to_bits()),
                3 => x87::st_f80(m, a, x87::st(c, 0)),
                4 => m.st16(a, x87::fist(c, 16) as u16),
                5 => m.st32(a, x87::fist(c, 32) as u32),
                _ => m.st64(a, x87::fist(c, 64)),
            }
            if pop != 0 {
                x87::fpop(c);
            }
        }
        3 => {
            let v = fsrc(c, m, code, a);
            x87::farith(c, sub, dst, v);
            if pop != 0 {
                x87::fpop(c);
            }
        }
        4 => x87::funary(c, sub),
        5 => x87::fbinary(c, sub),
        6 => {
            let v = fsrc(c, m, code, a);
            x87::fcom(c, v);
            for _ in 0..pop {
                x87::fpop(c);
            }
        }
        7 => x87::fxch(c, st),
        8 => x87::ffree(c, st),
        9 => c.top = c.top.wrapping_add(1) & 7,
        10 => c.top = c.top.wrapping_sub(1) & 7,
        11 => x87::finit(c),
        12 => c.fpu_status &= 0x7f00,
        13 => c.fpu_control = a as u16,
        14 => x87::fstenv(c, m, a),
        15 => x87::fldenv(c, m, a),
        16 => x87::fsave(c, m, a),
        17 => x87::frstor(c, m, a),
        _ => dispatch::fatal(&format!("native code: x87 operation {code:#x}")),
    }
}

unsafe extern "C" fn h_fstsw(c: *mut Cpu) -> u32 {
    let c = cpu(c);
    u32::from(x87::fstsw(c))
}

unsafe extern "C" fn h_point(c: *mut Cpu, addr: u32) {
    let c = cpu(c);
    dispatch::point(c, addr);
}

unsafe extern "C" fn h_mmx(_c: *mut Cpu, op: u32, a: u64, b: u64) -> u64 {
    use crate::mmx::*;
    // rc_ir::MmxOp in declaration order.
    const OPS: [fn(u64, u64) -> u64; 40] = [
        mmx_paddb,
        mmx_paddw,
        mmx_paddd,
        mmx_paddsb,
        mmx_paddsw,
        mmx_paddusb,
        mmx_paddusw,
        mmx_psubb,
        mmx_psubw,
        mmx_psubd,
        mmx_psubsb,
        mmx_psubsw,
        mmx_psubusb,
        mmx_psubusw,
        mmx_pmullw,
        mmx_pmulhw,
        mmx_pmaddwd,
        mmx_pcmpeqb,
        mmx_pcmpeqw,
        mmx_pcmpeqd,
        mmx_pcmpgtb,
        mmx_pcmpgtw,
        mmx_pcmpgtd,
        mmx_packsswb,
        mmx_packssdw,
        mmx_packuswb,
        mmx_punpcklbw,
        mmx_punpcklwd,
        mmx_punpckldq,
        mmx_punpckhbw,
        mmx_punpckhwd,
        mmx_punpckhdq,
        mmx_psllw,
        mmx_pslld,
        mmx_psllq,
        mmx_psrlw,
        mmx_psrld,
        mmx_psrlq,
        mmx_psraw,
        mmx_psrad,
    ];
    match OPS.get(op as usize) {
        Some(f) => f(a, b),
        None => dispatch::fatal(&format!("native code: MMX operation {op}")),
    }
}

unsafe extern "C" fn h_dispatch_jump(c: *mut Cpu, target: u32) {
    let c = cpu(c);
    dispatch::dispatch_jump(c, target);
}

unsafe extern "C" fn h_unwind_claim(c: *mut Cpu, ebp: u32, lo: u32, hi: u32) -> u32 {
    let c = cpu(c);
    dispatch::unwind_claim(c, ebp, lo, hi, &[]);
    u32::from(c.unwind == 0)
}

unsafe extern "C" fn h_unwind_begin(c: *mut Cpu, eip: u32) {
    let c = cpu(c);
    dispatch::unwind_begin(c, eip);
}

struct Helpers([*const (); 11]);
// SAFETY: a table of function addresses, never written after it is built.
unsafe impl Sync for Helpers {}

static HELPERS: Helpers = Helpers([
    h_dispatch as *const (),
    h_trap as *const (),
    h_cpuid as *const (),
    h_rdtsc as *const (),
    h_fpu as *const (),
    h_fstsw as *const (),
    h_point as *const (),
    h_mmx as *const (),
    h_dispatch_jump as *const (),
    h_unwind_claim as *const (),
    h_unwind_begin as *const (),
]);

/// Give a CPU the helper table, so native code can run on it.
pub fn bind(c: &mut Cpu) {
    c.helpers = HELPERS.0.as_ptr();
}

fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// A loaded blob: the modules (DLL names) whose code it holds.
pub struct Loaded {
    pub modules: Vec<String>,
    pub functions: usize,
}

/// Load a blob of native code: map it executable and add its functions to the dispatcher.
pub fn load(b: &[u8]) -> Result<Loaded, String> {
    if b.len() < 48 || &b[..8] != b"RCNATIVE" {
        return Err("not native code from recomp".into());
    }
    if rd32(b, 8) != 2 {
        return Err(format!(
            "native code of format {} (this runtime reads 2)",
            rd32(b, 8)
        ));
    }
    if rd32(b, 12) != host_target() {
        return Err(format!(
            "native code for another machine (target {}, this is {})",
            rd32(b, 12),
            host_target()
        ));
    }
    if rd32(b, 16) != rc_abi::cpu::MEM_OFFSET as u32
        || rd32(b, 20) != rc_abi::cpu::HELPERS_OFFSET as u32
    {
        return Err("native code for another runtime version (CPU state layout differs)".into());
    }
    let (nfuncs, ntraps, nmods) = (
        rd32(b, 24) as usize,
        rd32(b, 28) as usize,
        rd32(b, 32) as usize,
    );
    let (code_at, csize) = (rd32(b, 36) as usize, rd32(b, 40) as usize);
    if code_at + csize > b.len() || code_at < 48 || nfuncs == 0 {
        return Err("native code is damaged (sizes)".into());
    }
    let mut map = memmap2::MmapMut::map_anon(csize.max(1)).map_err(|e| e.to_string())?;
    map[..csize].copy_from_slice(&b[code_at..code_at + csize]);
    let exec = map.make_exec().map_err(|e| e.to_string())?;
    let base = exec.as_ptr();
    let mut p = 48;
    let need = |p: usize, n: usize| -> Result<(), String> {
        if p + n > code_at {
            Err("native code is damaged (tables)".into())
        } else {
            Ok(())
        }
    };
    let mut table: Vec<(u32, NativeFn)> = Vec::with_capacity(nfuncs);
    for _ in 0..nfuncs {
        need(p, 8)?;
        let (addr, off) = (rd32(b, p), rd32(b, p + 4) as usize);
        p += 8;
        if off >= csize || table.last().is_some_and(|l| l.0 >= addr) {
            return Err("native code is damaged (function table)".into());
        }
        // SAFETY: `off` lies inside the executable mapping, which is never unmapped (it is
        // leaked below), and the converter put a function of this signature there.
        let f: NativeFn = unsafe { std::mem::transmute::<*const u8, NativeFn>(base.add(off)) };
        table.push((addr, f));
    }
    let mut traps = TRAPS.lock().expect("traps");
    for _ in 0..ntraps {
        need(p, 12)?;
        let len = rd32(b, p + 8) as usize;
        need(p, 12 + len)?;
        traps.push((
            rd32(b, p),
            rd32(b, p + 4),
            String::from_utf8_lossy(&b[p + 12..p + 12 + len]).into(),
        ));
        p += 12 + len;
    }
    drop(traps);
    let mut modules = Vec::new();
    for _ in 0..nmods {
        need(p, 4)?;
        let len = rd32(b, p) as usize;
        need(p, 4 + len)?;
        modules.push(String::from_utf8_lossy(&b[p + 4..p + 4 + len]).into());
        p += 4 + len;
    }
    std::mem::forget(exec);
    let functions = table.len();
    dispatch::add_native(Box::leak(table.into_boxed_slice()));
    Ok(Loaded { modules, functions })
}

// SPDX-License-Identifier: MIT
//! Indirect calls and jumps: patches first, then the sorted tables of the lifted binaries, then
//! the import resolver (reserved import addresses → HLE). As the sister projects' `dispatch.c`.

use crate::cpu::{Cpu, EBP, ESP};
use std::cell::RefCell;
use std::sync::RwLock;

/// A lifted (or hand-written) guest function.
pub type GuestFn = fn(&mut Cpu);

/// Finds the HLE function behind a reserved import address.
pub type ImportResolver = fn(u32) -> Option<GuestFn>;

#[derive(Default)]
struct Registry {
    tables: Vec<&'static [(u32, GuestFn)]>,
    hooks: Vec<(u32, GuestFn)>,
    points: Vec<(u32, GuestFn)>,
    resolver: Option<ImportResolver>,
    generation: u64,
}

static REGISTRY: RwLock<Registry> = RwLock::new(Registry {
    tables: Vec::new(),
    hooks: Vec::new(),
    points: Vec::new(),
    resolver: None,
    generation: 0,
});

const CACHE: usize = 4096;

/// A per-thread lookup cache: the registry generation it is valid for, and its slots.
type Cache = (u64, Vec<(u32, Option<GuestFn>)>);

thread_local! {
    static LOOKUP: RefCell<Cache> = RefCell::new((u64::MAX, vec![(0, None); CACHE]));
}

fn changed(r: &mut Registry) {
    r.generation += 1;
}

/// Add a lifted binary's table (sorted by address).
pub fn add_lifted(table: &'static [(u32, GuestFn)]) {
    let mut r = REGISTRY.write().expect("registry");
    r.tables.push(table);
    changed(&mut r);
}

/// Remove every table, hook, point and the resolver (tests).
pub fn reset() {
    let mut r = REGISTRY.write().expect("registry");
    r.tables.clear();
    r.hooks.clear();
    r.points.clear();
    r.resolver = None;
    changed(&mut r);
}

pub fn set_import_resolver(f: ImportResolver) {
    let mut r = REGISTRY.write().expect("registry");
    r.resolver = Some(f);
    changed(&mut r);
}

/// Replace the function at `addr` (a game patch); returns the one it replaces, if any hook did.
pub fn hook(addr: u32, f: GuestFn) -> Option<GuestFn> {
    let mut r = REGISTRY.write().expect("registry");
    changed(&mut r);
    if let Some(h) = r.hooks.iter_mut().find(|h| h.0 == addr) {
        return Some(std::mem::replace(&mut h.1, f));
    }
    r.hooks.push((addr, f));
    None
}

/// Run `f` when lifted code passes patch point `addr`.
pub fn on_point(addr: u32, f: GuestFn) {
    let mut r = REGISTRY.write().expect("registry");
    if let Some(p) = r.points.iter_mut().find(|p| p.0 == addr) {
        p.1 = f;
    } else {
        r.points.push((addr, f));
    }
    changed(&mut r);
}

pub fn point(c: &mut Cpu, addr: u32) {
    let f = {
        let r = REGISTRY.read().expect("registry");
        r.points.iter().find(|p| p.0 == addr).map(|p| p.1)
    };
    if let Some(f) = f {
        c.eip = addr;
        f(c);
    }
}

fn resolve(addr: u32) -> Option<GuestFn> {
    let r = REGISTRY.read().expect("registry");
    if let Some(h) = r.hooks.iter().find(|h| h.0 == addr) {
        return Some(h.1);
    }
    for t in &r.tables {
        if let Ok(i) = t.binary_search_by_key(&addr, |e| e.0) {
            return Some(t[i].1);
        }
    }
    r.resolver.and_then(|f| f(addr))
}

/// The function at `addr`: a hook, a lifted function or an import.
pub fn lookup(addr: u32) -> Option<GuestFn> {
    let generation = REGISTRY.read().expect("registry").generation;
    LOOKUP.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.0 != generation {
            cache.0 = generation;
            cache.1.iter_mut().for_each(|e| *e = (0, None));
        }
        let slot = ((addr ^ (addr >> 12)) as usize) & (CACHE - 1);
        if let (a, Some(f)) = cache.1[slot] {
            if a == addr {
                return Some(f);
            }
        }
        let f = resolve(addr);
        if f.is_some() {
            cache.1[slot] = (addr, f);
        }
        f
    })
}

/// An indirect call: the return address is already pushed.
pub fn dispatch(c: &mut Cpu, target: u32) {
    match lookup(target) {
        Some(f) => f(c),
        None => {
            c.eip = target;
            let ret = c.mem().ld32(c.r[ESP]);
            fatal(&format!(
                "indirect transfer to {target:#010x}, which is not a lifted function (esp {:#010x}, return address {ret:#010x})",
                c.r[ESP]
            ));
        }
    }
}

/// An indirect jump: during an exception a jump out of the handler's frame continues there.
pub fn dispatch_jump(c: &mut Cpu, target: u32) {
    if c.eh_depth != 0 && c.r[ESP] > c.eh_esp {
        c.unwind = 1;
        c.unwind_eip = target;
        c.unwind_ebp = c.r[EBP];
        c.eip = target;
        return;
    }
    dispatch(c, target);
}

/// After a call returned while unwinding: the frame with `ebp` whose code holds the place to
/// continue at (`lo..hi` or one of `at`) takes it over.
pub fn unwind_claim(c: &mut Cpu, ebp: u32, lo: u32, hi: u32, at: &[u32]) {
    if c.unwind_ebp != ebp {
        return;
    }
    let mine = (lo..hi).contains(&c.unwind_eip) || at.contains(&c.unwind_eip);
    if !mine {
        return;
    }
    c.unwind = 0;
    c.resume_eip = c.unwind_eip;
    let target = c.unwind_eip;
    dispatch(c, target);
    c.resume_eip = 0;
}

pub fn unwind_begin(c: &mut Cpu, eip: u32) {
    c.unwind = 1;
    c.unwind_eip = eip;
    c.unwind_ebp = c.r[EBP];
    c.eip = eip;
}

/// Stop with a message (the process cannot go on).
pub fn fatal(msg: &str) -> ! {
    panic!("fatal: {msg}");
}

/// A lifted instruction the runtime cannot execute.
pub fn trap(c: &mut Cpu, addr: u32, why: &str) -> ! {
    c.eip = addr;
    let r = &c.r;
    fatal(&format!(
        "{why} (at {addr:#010x}; eax {:#x} ecx {:#x} edx {:#x} ebx {:#x} esp {:#x} ebp {:#x} esi {:#x} edi {:#x})",
        r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7]
    ));
}

/// A P6-class CPU with FPU, TSC, CX8, CMOV and MMX, as the sister projects report.
pub fn cpuid(eax: &mut u32, ebx: &mut u32, ecx: &mut u32, edx: &mut u32) {
    match *eax {
        0 => {
            *eax = 1;
            *ebx = 0x756e_6547;
            *edx = 0x4965_6e69;
            *ecx = 0x6c65_746e;
        }
        1 => {
            *eax = 0x0000_0683;
            *ebx = 0;
            *ecx = 0;
            *edx = 0x0080_8111;
        }
        _ => {
            *eax = 0;
            *ebx = 0;
            *ecx = 0;
            *edx = 0;
        }
    }
}

/// The time-stamp counter: nanoseconds (a 1 GHz CPU); on the virtual clock its time plus a
/// step per reading, so that it always grows.
pub fn rdtsc(eax: &mut u32, edx: &mut u32) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static READS: AtomicU64 = AtomicU64::new(0);
    let mut t = crate::clock::ticks_ns();
    if crate::clock::is_virtual() {
        t += READS.fetch_add(1, Ordering::Relaxed) % 1_000_000;
    }
    *eax = t as u32;
    *edx = (t >> 32) as u32;
}

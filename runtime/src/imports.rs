// SPDX-License-Identifier: MIT
//! Imported functions: every one gets a reserved guest address (`0xf0000000` on, 16 bytes
//! apart), so the game's `call [IAT]` and its vtable calls on COM objects land in the
//! dispatcher, which runs the HLE function registered for that DLL and name, or stops with
//! `unimplemented import dll!Name`. As the sister projects' `imports.c`.

use crate::cpu::{Cpu, EAX, ESP};
use crate::dispatch::{fatal, GuestFn};
use std::sync::RwLock;

pub const IMPORT_BASE: u32 = 0xf000_0000;
pub const IMPORT_STRIDE: u32 = 16;
pub const IMPORT_MAX: u32 = 8192;

struct Slot {
    dll: String,
    name: String,
    f: Option<GuestFn>,
}

struct Registry {
    /// HLE functions by (dll without `.dll`, lower case; name).
    defs: Vec<(String, String, GuestFn)>,
    slots: Vec<Slot>,
    trace: bool,
}

static REG: RwLock<Registry> = RwLock::new(Registry {
    defs: Vec::new(),
    slots: Vec::new(),
    trace: false,
});

fn dll_key(dll: &str) -> String {
    let d = dll.to_ascii_lowercase();
    d.strip_suffix(".dll").map(str::to_string).unwrap_or(d)
}

/// Register HLE functions of one DLL.
pub fn register(dll: &str, defs: &[(&str, GuestFn)]) {
    let mut r = REG.write().expect("imports");
    let key = dll_key(dll);
    for (name, f) in defs {
        r.defs.push((key.clone(), (*name).to_string(), *f));
    }
    // Slots handed out before this registration get their function now.
    let defs: Vec<(String, String, GuestFn)> = r.defs.clone();
    for s in &mut r.slots {
        if s.f.is_none() {
            s.f = defs
                .iter()
                .find(|d| d.0 == dll_key(&s.dll) && d.1 == s.name)
                .map(|d| d.2);
        }
    }
}

/// Log every import call (`RC_TRACE=imports`).
pub fn set_trace(on: bool) {
    REG.write().expect("imports").trace = on;
}

/// Whether HLE implements `dll!name`.
pub fn implemented(dll: &str, name: &str) -> bool {
    let key = dll_key(dll);
    REG.read()
        .expect("imports")
        .defs
        .iter()
        .any(|d| d.0 == key && d.1 == name)
}

/// The reserved address of `dll!name` (an ordinal is named `#N`), handed out on first use.
pub fn address(dll: &str, name: &str) -> u32 {
    let mut r = REG.write().expect("imports");
    let key = dll_key(dll);
    if let Some(i) = r
        .slots
        .iter()
        .position(|s| dll_key(&s.dll) == key && s.name == name)
    {
        return IMPORT_BASE + IMPORT_STRIDE * i as u32;
    }
    if r.slots.len() as u32 >= IMPORT_MAX {
        fatal("too many imports");
    }
    let f = r
        .defs
        .iter()
        .find(|d| d.0 == key && d.1 == name)
        .map(|d| d.2);
    r.slots.push(Slot {
        dll: dll.to_string(),
        name: name.to_string(),
        f,
    });
    IMPORT_BASE + IMPORT_STRIDE * (r.slots.len() as u32 - 1)
}

/// `dll!name` of a reserved address.
pub fn name_of(addr: u32) -> Option<(String, String)> {
    let i = addr.checked_sub(IMPORT_BASE)? / IMPORT_STRIDE;
    let r = REG.read().expect("imports");
    r.slots
        .get(i as usize)
        .map(|s| (s.dll.clone(), s.name.clone()))
}

thread_local! {
    static CURRENT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn traced(c: &mut Cpu) {
    let i = CURRENT.with(|x| x.get());
    let (dll, name, f) = {
        let r = REG.read().expect("imports");
        let s = &r.slots[i as usize];
        (s.dll.clone(), s.name.clone(), s.f)
    };
    let m = c.mem();
    let sp = c.r[ESP];
    eprintln!(
        "[import] {dll}!{name}({:#x}, {:#x}, {:#x}, {:#x}) from {:#010x}",
        m.ld32(sp + 4),
        m.ld32(sp + 8),
        m.ld32(sp + 12),
        m.ld32(sp + 16),
        m.ld32(sp)
    );
    if let Some(f) = f {
        f(c);
    }
    eprintln!("[import]   = {:#x}", c.r[EAX]);
}

/// The dispatcher's resolver for reserved addresses.
pub fn resolve(addr: u32) -> Option<GuestFn> {
    if !(IMPORT_BASE..IMPORT_BASE + IMPORT_STRIDE * IMPORT_MAX).contains(&addr) {
        return None;
    }
    let i = (addr - IMPORT_BASE) / IMPORT_STRIDE;
    let r = REG.read().expect("imports");
    let s = r.slots.get(i as usize)?;
    match s.f {
        Some(_) if r.trace => {
            CURRENT.with(|x| x.set(i));
            Some(traced)
        }
        Some(f) => Some(f),
        None => fatal(&format!("unimplemented import {}!{}", s.dll, s.name)),
    }
}

/// Every import handed out that HLE does not implement (`--check`).
pub fn missing() -> Vec<String> {
    let r = REG.read().expect("imports");
    r.slots
        .iter()
        .filter(|s| s.f.is_none())
        .map(|s| format!("{}!{}", s.dll, s.name))
        .collect()
}

/// Return from an stdcall HLE function: result in `eax`, pop the return address and `args`
/// 32-bit arguments.
pub fn ret_stdcall(c: &mut Cpu, result: u32, args: u32) {
    c.r[EAX] = result;
    c.r[ESP] = c.r[ESP].wrapping_add(4 + 4 * args);
}

/// Return from a cdecl HLE function: the caller pops the arguments.
pub fn ret_cdecl(c: &mut Cpu, result: u32) {
    c.r[EAX] = result;
    c.r[ESP] = c.r[ESP].wrapping_add(4);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seven(c: &mut Cpu) {
        ret_stdcall(c, 7, 0);
    }

    #[test]
    fn addresses_are_stable_and_resolve() {
        let a = address("TEST.DLL", "Seven");
        assert_eq!(address("test", "Seven"), a);
        assert!(!implemented("test", "Seven"));
        register("test.dll", &[("Seven", seven)]);
        assert!(resolve(a).is_some());
        assert_eq!(name_of(a), Some(("TEST.DLL".into(), "Seven".into())));
    }
}

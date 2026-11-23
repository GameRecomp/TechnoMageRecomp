// SPDX-License-Identifier: MIT
//! COM objects for DirectX: an object lives in guest memory as `[vtable][reference count]
//! [host id]`; its vtable holds the reserved import addresses of `dll!Interface::Method`, so the
//! game's own vtable calls land in the HLE methods, and a method that is missing stops with its
//! name (`unimplemented import ddraw.dll!IDirectDrawSurface7::Flip`).

use rc_runtime::cpu::Cpu;
use rc_runtime::dispatch::GuestFn;
use rc_runtime::imports::{self, ret_stdcall};
use rc_runtime::mem::Mem;
use rc_runtime::vm;
use std::collections::BTreeMap;
use std::sync::Mutex;

pub const S_OK: u32 = 0;
pub const E_NOINTERFACE: u32 = 0x8000_4002;
pub const E_NOTIMPL: u32 = 0x8000_4001;

/// An interface: its name, its methods in vtable order (`None`: not implemented yet, the slot
/// still gets a named address).
pub struct Interface {
    pub dll: &'static str,
    pub name: &'static str,
    pub methods: &'static [(&'static str, Option<GuestFn>)],
}

static VTABLES: Mutex<BTreeMap<&'static str, u32>> = Mutex::new(BTreeMap::new());

/// The guest vtable of `iface`, built and registered on first use.
pub fn vtable(m: Mem, iface: &Interface) -> u32 {
    if let Some(&v) = VTABLES.lock().expect("com").get(iface.name) {
        return v;
    }
    let mut defs: Vec<(String, GuestFn)> = Vec::new();
    for (meth, f) in iface.methods {
        if let Some(f) = f {
            defs.push((format!("{}::{meth}", iface.name), *f));
        }
    }
    let leaked: Vec<(&'static str, GuestFn)> = defs
        .into_iter()
        .map(|(n, f)| (&*Box::leak(n.into_boxed_str()), f))
        .collect();
    imports::register(iface.dll, &leaked);
    let v = vm::heap_alloc(m, 4 * iface.methods.len() as u32, false);
    for (i, (meth, _)) in iface.methods.iter().enumerate() {
        let a = imports::address(iface.dll, &format!("{}::{meth}", iface.name));
        m.st32(v + 4 * i as u32, a);
    }
    VTABLES.lock().expect("com").insert(iface.name, v);
    v
}

/// A new object of `iface` whose host state is `id`, with one reference.
pub fn new_object(m: Mem, iface: &Interface, id: u32) -> u32 {
    let vt = vtable(m, iface);
    let o = vm::heap_alloc(m, 16, true);
    m.st32(o, vt);
    m.st32(o + 4, 1);
    m.st32(o + 8, id);
    o
}

/// The host id of the object `this`.
pub fn id_of(m: Mem, this: u32) -> u32 {
    m.ld32(this + 8)
}

/// One more reference to `this` (an interface handed out again).
pub fn add_ref_raw(m: Mem, this: u32) {
    m.st32(this + 4, m.ld32(this + 4) + 1);
}

/// `AddRef` for every interface: count up, return the count.
pub fn add_ref(c: &mut Cpu) {
    let m = c.mem();
    let this = c.arg(0);
    let n = m.ld32(this + 4) + 1;
    m.st32(this + 4, n);
    ret_stdcall(c, n, 1);
}

/// `Release` for objects whose host state outlives the last reference (kept for simplicity):
/// count down, return the count.
pub fn release(c: &mut Cpu) {
    let m = c.mem();
    let this = c.arg(0);
    let n = m.ld32(this + 4).saturating_sub(1);
    m.st32(this + 4, n);
    ret_stdcall(c, n, 1);
}

/// A GUID as the bytes Windows stores (`Data1`–`Data3` little-endian).
pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

/// The GUID at a guest address.
pub fn read_guid(m: Mem, a: u32) -> [u8; 16] {
    let mut g = [0u8; 16];
    m.read(a, &mut g);
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_layout() {
        // IID_IDirectDraw7 {15e65ec0-3b9c-11d2-b92f-00609797ea5b}
        let g = guid(
            0x15e6_5ec0,
            0x3b9c,
            0x11d2,
            [0xb9, 0x2f, 0x00, 0x60, 0x97, 0x97, 0xea, 0x5b],
        );
        assert_eq!(&g[..4], &[0xc0, 0x5e, 0xe6, 0x15]);
        assert_eq!(g[15], 0x5b);
    }
}

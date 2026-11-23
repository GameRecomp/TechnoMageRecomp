// SPDX-License-Identifier: MIT
//! The small modules: gdi32 (the gamma ramp), ole32 (COM start-up and `CoCreateInstance`),
//! winmm (`timeGetTime`).

#![allow(non_snake_case)]

use rc_runtime::cpu::Cpu;
use rc_runtime::imports::{self, ret_stdcall};
use std::sync::Mutex;

/// The gamma ramp the game set (red, green, blue; 256 entries each), for the present shader.
static GAMMA: Mutex<Option<Vec<u16>>> = Mutex::new(None);

pub fn gamma_ramp() -> Option<Vec<u16>> {
    GAMMA.lock().expect("gamma").clone()
}

fn GetDeviceGammaRamp(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let ramp = gamma_ramp();
    for i in 0..768u32 {
        let v = match &ramp {
            Some(r) => r[i as usize],
            None => ((i % 256) * 257) as u16,
        };
        m.st16(p + 2 * i, v);
    }
    ret_stdcall(c, 1, 2);
}

fn SetDeviceGammaRamp(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let r: Vec<u16> = (0..768u32).map(|i| m.ld16(p + 2 * i)).collect();
    *GAMMA.lock().expect("gamma") = Some(r);
    ret_stdcall(c, 1, 2);
}

fn CoInitialize(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

fn CoUninitialize(c: &mut Cpu) {
    ret_stdcall(c, 0, 0);
}

/// A COM class an HLE module provides: its CLSID and a constructor that returns the interface
/// asked for by IID (the guest address of the object), or 0.
pub type ComFactory = fn(&mut Cpu, iid: [u8; 16]) -> u32;

static CLASSES: Mutex<Vec<([u8; 16], ComFactory)>> = Mutex::new(Vec::new());

pub fn register_class(clsid: [u8; 16], f: ComFactory) {
    CLASSES.lock().expect("com").push((clsid, f));
}

fn CoCreateInstance(c: &mut Cpu) {
    let m = c.mem();
    let (clsid_p, iid_p, out) = (c.arg(0), c.arg(3), c.arg(4));
    let mut clsid = [0u8; 16];
    let mut iid = [0u8; 16];
    m.read(clsid_p, &mut clsid);
    m.read(iid_p, &mut iid);
    let f = CLASSES
        .lock()
        .expect("com")
        .iter()
        .find(|x| x.0 == clsid)
        .map(|x| x.1);
    let r = match f {
        Some(f) => {
            let obj = f(c, iid);
            m.st32(out, obj);
            if obj != 0 {
                0
            } else {
                0x8000_4002 // E_NOINTERFACE
            }
        }
        None => {
            m.st32(out, 0);
            eprintln!("[ole32] CoCreateInstance: class {clsid:02x?} not provided");
            0x8004_0154 // REGDB_E_CLASSNOTREG
        }
    };
    ret_stdcall(c, r, 5);
}

fn timeGetTime(c: &mut Cpu) {
    ret_stdcall(c, rc_runtime::clock::ticks_ms() as u32, 0);
}

pub fn register() {
    imports::register(
        "gdi32.dll",
        &[
            (
                "GetDeviceGammaRamp",
                GetDeviceGammaRamp as rc_runtime::dispatch::GuestFn,
            ),
            ("SetDeviceGammaRamp", SetDeviceGammaRamp),
        ],
    );
    imports::register(
        "ole32.dll",
        &[
            (
                "CoInitialize",
                CoInitialize as rc_runtime::dispatch::GuestFn,
            ),
            ("CoUninitialize", CoUninitialize),
            ("CoCreateInstance", CoCreateInstance),
        ],
    );
    imports::register(
        "winmm.dll",
        &[("timeGetTime", timeGetTime as rc_runtime::dispatch::GuestFn)],
    );
}

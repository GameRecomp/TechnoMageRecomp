// SPDX-License-Identifier: MIT
//! dinput: DirectInput (`DirectInputCreateA` through `LoadLibraryA`) with the system keyboard
//! and mouse. Their state comes from the platform ([`key`], [`mouse_motion`], [`mouse_button`]);
//! `GetDeviceState` reads it, mouse motion as the deltas since the last read.

#![allow(non_snake_case)]

use crate::com::{self, Interface, E_NOINTERFACE, S_OK};
use rc_runtime::cpu::Cpu;
use rc_runtime::imports::{self, ret_stdcall};
use std::sync::Mutex;

const GUID_SYSMOUSE: [u8; 16] = com::guid(
    0x6f1d_2b60,
    0xd5a0,
    0x11cf,
    [0xbf, 0xc7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);
const GUID_SYSKEYBOARD: [u8; 16] = com::guid(
    0x6f1d_2b61,
    0xd5a0,
    0x11cf,
    [0xbf, 0xc7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);
const DIERR_NOTINITIALIZED: u32 = 0x8007_0015;

const KEYBOARD: u32 = 1;
const MOUSE: u32 = 2;

struct State {
    keys: [u8; 256],
    dx: i32,
    dy: i32,
    dz: i32,
    buttons: [u8; 4],
}

static STATE: Mutex<State> = Mutex::new(State {
    keys: [0; 256],
    dx: 0,
    dy: 0,
    dz: 0,
    buttons: [0; 4],
});

/// A key (DirectInput scan code `DIK_*`) went down or up.
pub fn key(scan: u8, down: bool) {
    STATE.lock().expect("dinput").keys[usize::from(scan)] = if down { 0x80 } else { 0 };
}

/// The mouse moved (relative, in mickeys).
pub fn mouse_motion(dx: i32, dy: i32, dz: i32) {
    let mut s = STATE.lock().expect("dinput");
    s.dx += dx;
    s.dy += dy;
    s.dz += dz;
}

/// A mouse button (0 left, 1 right, 2 middle) went down or up.
pub fn mouse_button(b: usize, down: bool) {
    if b < 4 {
        STATE.lock().expect("dinput").buttons[b] = if down { 0x80 } else { 0 };
    }
}

fn DirectInputCreateA(c: &mut Cpu) {
    let m = c.mem();
    let out = c.arg(2);
    m.st32(out, com::new_object(m, &DINPUT, 0));
    ret_stdcall(c, S_OK, 4);
}

fn di_CreateDevice(c: &mut Cpu) {
    let m = c.mem();
    let (g, out) = (com::read_guid(m, c.arg(1)), c.arg(2));
    let kind = if g == GUID_SYSKEYBOARD {
        KEYBOARD
    } else if g == GUID_SYSMOUSE {
        MOUSE
    } else {
        m.st32(out, 0);
        return ret_stdcall(c, E_NOINTERFACE, 4);
    };
    m.st32(out, com::new_object(m, &DEVICE, kind));
    ret_stdcall(c, S_OK, 4);
}

fn ok1(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 1);
}
fn ok2(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 2);
}
fn ok3(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 3);
}

fn dev_QueryInterface(c: &mut Cpu) {
    // Device2 and Device7 extend Device: the same object serves them.
    let m = c.mem();
    let (this, out) = (c.arg(0), c.arg(2));
    com::add_ref_raw(m, this);
    m.st32(out, this);
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetDeviceState(c: &mut Cpu) {
    let m = c.mem();
    let (this, size, out) = (c.arg(0), c.arg(1), c.arg(2));
    let kind = com::id_of(m, this);
    let mut s = STATE.lock().expect("dinput");
    match kind {
        KEYBOARD => {
            let n = size.min(256) as usize;
            m.write(out, &s.keys[..n]);
        }
        MOUSE => {
            m.st32(out, s.dx as u32);
            m.st32(out + 4, s.dy as u32);
            m.st32(out + 8, s.dz as u32);
            m.write(out + 12, &s.buttons);
            for i in 16..size {
                m.st8(out + i, 0);
            }
            s.dx = 0;
            s.dy = 0;
            s.dz = 0;
        }
        _ => {
            drop(s);
            return ret_stdcall(c, DIERR_NOTINITIALIZED, 3);
        }
    }
    drop(s);
    ret_stdcall(c, S_OK, 3);
}

fn dev_GetDeviceData(c: &mut Cpu) {
    // No buffered events: the games read the state.
    let m = c.mem();
    let n = c.arg(3);
    if n != 0 {
        m.st32(n, 0);
    }
    ret_stdcall(c, S_OK, 5);
}

static DINPUT: Interface = Interface {
    dll: "dinput.dll",
    name: "IDirectInputA",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("CreateDevice", Some(di_CreateDevice)),
        ("EnumDevices", None),
        ("GetDeviceStatus", Some(ok2)),
        ("RunControlPanel", None),
        ("Initialize", Some(ok3)),
    ],
};

static DEVICE: Interface = Interface {
    dll: "dinput.dll",
    name: "IDirectInputDeviceA",
    methods: &[
        ("QueryInterface", Some(dev_QueryInterface)),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("GetCapabilities", None),
        ("EnumObjects", None),
        ("GetProperty", None),
        ("SetProperty", Some(ok3)),
        ("Acquire", Some(ok1)),
        ("Unacquire", Some(ok1)),
        ("GetDeviceState", Some(dev_GetDeviceState)),
        ("GetDeviceData", Some(dev_GetDeviceData)),
        ("SetDataFormat", Some(ok2)),
        ("SetEventNotification", Some(ok2)),
        ("SetCooperativeLevel", Some(ok3)),
        ("GetObjectInfo", None),
        ("GetDeviceInfo", None),
        ("RunControlPanel", None),
        ("Initialize", None),
        ("CreateEffect", None),
        ("EnumEffects", None),
        ("GetEffectInfo", None),
        ("GetForceFeedbackState", None),
        ("SendForceFeedbackCommand", None),
        ("EnumCreatedEffectObjects", None),
        ("Escape", None),
        ("Poll", Some(ok1)),
        ("SendDeviceData", None),
    ],
};

pub fn register() {
    imports::register(
        "dinput.dll",
        &[(
            "DirectInputCreateA",
            DirectInputCreateA as rc_runtime::dispatch::GuestFn,
        )],
    );
}

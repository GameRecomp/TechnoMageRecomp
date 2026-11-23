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
    /// Buffered events (`GetDeviceData`): offset, data, time; per device kind.
    key_events: std::collections::VecDeque<(u32, u32, u32)>,
    mouse_events: std::collections::VecDeque<(u32, u32, u32)>,
    sequence: u32,
    keys: [u8; 256],
    dx: i32,
    dy: i32,
    dz: i32,
    buttons: [u8; 4],
}

static STATE: Mutex<State> = Mutex::new(State {
    key_events: std::collections::VecDeque::new(),
    mouse_events: std::collections::VecDeque::new(),
    sequence: 0,
    keys: [0; 256],
    dx: 0,
    dy: 0,
    dz: 0,
    buttons: [0; 4],
});

/// A key (DirectInput scan code `DIK_*`) went down or up.
pub fn key(scan: u8, down: bool) {
    let mut s = STATE.lock().expect("dinput");
    let v = if down { 0x80 } else { 0 };
    s.keys[usize::from(scan)] = v;
    let t = rc_runtime::clock::ticks_ms() as u32;
    s.key_events.push_back((u32::from(scan), u32::from(v), t));
}

/// The mouse moved (relative, in mickeys).
pub fn mouse_motion(dx: i32, dy: i32, dz: i32) {
    let mut s = STATE.lock().expect("dinput");
    s.dx += dx;
    s.dy += dy;
    s.dz += dz;
    let t = rc_runtime::clock::ticks_ms() as u32;
    for (ofs, d) in [(0u32, dx), (4, dy), (8, dz)] {
        if d != 0 {
            s.mouse_events.push_back((ofs, d as u32, t));
        }
    }
}

/// A mouse button (0 left, 1 right, 2 middle) went down or up.
pub fn mouse_button(b: usize, down: bool) {
    if b < 4 {
        let mut s = STATE.lock().expect("dinput");
        let v = if down { 0x80 } else { 0 };
        s.buttons[b] = v;
        let t = rc_runtime::clock::ticks_ms() as u32;
        s.mouse_events.push_back((12 + b as u32, u32::from(v), t));
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

/// The system keyboard and mouse, as DIDEVICEINSTANCEA for an `EnumDevices` callback; no
/// joysticks.
fn di_EnumDevices(c: &mut Cpu) {
    let m = c.mem();
    let (kind, cb, ctx) = (c.arg(1), c.arg(2), c.arg(3));
    let devices: &[([u8; 16], u32, &[u8])] = &[
        (GUID_SYSKEYBOARD, 0x0403, b"Keyboard\0"),
        (GUID_SYSMOUSE, 0x0202, b"Mouse\0"),
    ];
    let inst = rc_runtime::vm::heap_alloc(m, 0x244, true);
    for (g, t, name) in devices {
        if kind != 0 && kind != (t & 0xff) {
            continue;
        }
        for i in (0..0x244).step_by(4) {
            m.st32(inst + i, 0);
        }
        m.st32(inst, 0x244);
        m.write(inst + 4, g);
        m.write(inst + 20, g);
        m.st32(inst + 36, *t);
        m.write(inst + 40, name);
        m.write(inst + 300, name);
        if rc_runtime::dispatch::call_guest(c, cb, &[inst, ctx]) == 0 {
            break;
        }
    }
    rc_runtime::vm::heap_free(inst);
    ret_stdcall(c, S_OK, 5);
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

/// Buffered input: DIDEVICEOBJECTDATA (offset, data, time stamp, sequence) records, as many as
/// the game has room for; `DIGDD_PEEK` (1) leaves them queued.
fn dev_GetDeviceData(c: &mut Cpu) {
    let m = c.mem();
    let (this, size, out, n_p, flags) = (c.arg(0), c.arg(1), c.arg(2), c.arg(3), c.arg(4));
    let kind = com::id_of(m, this);
    let room = if n_p != 0 { m.ld32(n_p) } else { 0 };
    let mut s = STATE.lock().expect("dinput");
    let mut given = 0u32;
    while given < room {
        let ev = match kind {
            KEYBOARD => s.key_events.get(given as usize).copied(),
            MOUSE => s.mouse_events.get(given as usize).copied(),
            _ => None,
        };
        let Some((ofs, data, t)) = ev else {
            break;
        };
        if out != 0 {
            let p = out + given * size.max(16);
            s.sequence += 1;
            m.st32(p, ofs);
            m.st32(p + 4, data);
            m.st32(p + 8, t);
            m.st32(p + 12, s.sequence);
        }
        given += 1;
    }
    if flags & 1 == 0 {
        let q = if kind == KEYBOARD {
            &mut s.key_events
        } else {
            &mut s.mouse_events
        };
        for _ in 0..given {
            q.pop_front();
        }
    }
    drop(s);
    if n_p != 0 {
        m.st32(n_p, given);
    }
    ret_stdcall(c, S_OK, 5);
}

/// The name of a key (by DirectInput scan code), as Windows' German/English keyboard names it.
fn key_name(scan: u32) -> String {
    const NAMES: &[(u32, &str)] = &[
        (0x01, "Esc"),
        (0x0e, "Backspace"),
        (0x0f, "Tab"),
        (0x1c, "Enter"),
        (0x1d, "Ctrl"),
        (0x2a, "Shift"),
        (0x36, "Right Shift"),
        (0x38, "Alt"),
        (0x39, "Space"),
        (0x3a, "Caps Lock"),
        (0xc8, "Up"),
        (0xd0, "Down"),
        (0xcb, "Left"),
        (0xcd, "Right"),
        (0xc9, "Page Up"),
        (0xd1, "Page Down"),
        (0xc7, "Home"),
        (0xcf, "End"),
        (0xd2, "Insert"),
        (0xd3, "Delete"),
        (0x9d, "Right Ctrl"),
        (0xb8, "Right Alt"),
    ];
    if let Some((_, n)) = NAMES.iter().find(|x| x.0 == scan) {
        return (*n).to_string();
    }
    if (0x3b..=0x44).contains(&scan) {
        return format!("F{}", scan - 0x3a);
    }
    const ROWS: &[(u32, &str)] = &[
        (0x02, "1234567890-="),
        (0x10, "QWERTZUIOP"),
        (0x1e, "ASDFGHJKL"),
        (0x2c, "YXCVBNM,.-"),
    ];
    for (start, keys) in ROWS {
        if let Some(ch) = scan
            .checked_sub(*start)
            .and_then(|i| keys.chars().nth(i as usize))
        {
            return ch.to_string();
        }
    }
    format!("Key {scan}")
}

/// DIDEVICEOBJECTINSTANCEA of a key or mouse object, by offset (`DIPH_BYOFFSET`).
fn dev_GetObjectInfo(c: &mut Cpu) {
    let m = c.mem();
    let (this, p, obj) = (c.arg(0), c.arg(1), c.arg(2));
    let size = m.ld32(p).clamp(32 + 260, 0x13c);
    for i in (4..size).step_by(4) {
        m.st32(p + i, 0);
    }
    m.st32(p + 20, obj);
    let name = if com::id_of(m, this) == KEYBOARD {
        key_name(obj)
    } else {
        match obj {
            0 => "X".into(),
            4 => "Y".into(),
            8 => "Wheel".into(),
            o => format!("Button {}", o.saturating_sub(11)),
        }
    };
    let mut b = name.into_bytes();
    b.push(0);
    m.write(p + 32, &b);
    ret_stdcall(c, S_OK, 4);
}

static DINPUT: Interface = Interface {
    dll: "dinput.dll",
    name: "IDirectInputA",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("CreateDevice", Some(di_CreateDevice)),
        ("EnumDevices", Some(di_EnumDevices)),
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
        ("GetObjectInfo", Some(dev_GetObjectInfo)),
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

const CLSID_DIRECTINPUT: [u8; 16] = com::guid(
    0x25e6_09e0,
    0xb259,
    0x11cf,
    [0xbf, 0xc7, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);

/// `CoCreateInstance(CLSID_DirectInput)`: the object, to be initialised by the game.
fn create(c: &mut Cpu, _iid: [u8; 16]) -> u32 {
    com::new_object(c.mem(), &DINPUT, 0)
}

pub fn register() {
    crate::small::register_class(CLSID_DIRECTINPUT, create);
    imports::register(
        "dinput.dll",
        &[(
            "DirectInputCreateA",
            DirectInputCreateA as rc_runtime::dispatch::GuestFn,
        )],
    );
}

// SPDX-License-Identifier: MIT
//! dinput: DirectInput (`DirectInputCreateA` through `LoadLibraryA`) with the system keyboard
//! and mouse and one joystick, the gamepads as a PlayStation pad. Their state comes from the
//! platform ([`key`], [`mouse_motion`], [`mouse_button`], [`pad`]); `GetDeviceState` reads it,
//! mouse motion as the deltas since the last read. The joystick is there whether a gamepad is
//! connected or not (the game opens it once, at its start), centred and with nothing pressed
//! while none is.

#![allow(non_snake_case)]

use crate::com::{self, Interface, E_NOINTERFACE, S_OK};
use rc_runtime::cpu::Cpu;
use rc_runtime::imports::{self, ret_stdcall};
use rc_runtime::mem::Mem;
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
/// The joystick's instance and product (made up, as a driver's would be).
const GUID_PAD: [u8; 16] = com::guid(
    0x5ec0_7e00,
    0x7e1e,
    0x4a6e,
    [0x9a, 0x0e, 0x50, 0x53, 0x31, 0x50, 0x41, 0x44],
);
const DIERR_NOTINITIALIZED: u32 = 0x8007_0015;
const DIERR_UNSUPPORTED: u32 = 0x8000_4001;
/// `DIDEVTYPE_JOYSTICK` with `DIDEVTYPEJOYSTICK_GAMEPAD` and `DIDEVTYPE_HID`.
const PAD_TYPE: u32 = 0x0001_0204;
/// The joystick's axis range (DirectInput's default).
const AXIS_MAX: u32 = 0xffff;

const KEYBOARD: u32 = 1;
const MOUSE: u32 = 2;
const JOYSTICK: u32 = 3;

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
    /// The pad: its buttons (`rc_platform::pad`) and direction.
    pad: (u16, i16, i16),
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
    pad: (0, 0, 0),
});

/// The gamepads changed: the buttons held and the direction (-32768 to 32767, y down).
pub fn pad(buttons: u16, x: i16, y: i16) {
    STATE.lock().expect("dinput").pad = (buttons, x, y);
}

/// The pad's buttons held now (`rc_platform::pad`).
pub fn pad_buttons() -> u16 {
    STATE.lock().expect("dinput").pad.0
}

/// Whether a key (DirectInput scan code `DIK_*`) is held.
pub fn key_down(scan: u8) -> bool {
    STATE.lock().expect("dinput").keys[usize::from(scan)] & 0x80 != 0
}

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

/// Whether mouse button `b` (0 left) is held.
pub fn mouse_button_down(b: usize) -> bool {
    STATE
        .lock()
        .expect("dinput")
        .buttons
        .get(b)
        .is_some_and(|&v| v & 0x80 != 0)
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
    } else if g == GUID_PAD {
        JOYSTICK
    } else {
        m.st32(out, 0);
        return ret_stdcall(c, E_NOINTERFACE, 4);
    };
    m.st32(out, com::new_object(m, &DEVICE, kind));
    ret_stdcall(c, S_OK, 4);
}

/// The system keyboard and mouse and the pad, as DIDEVICEINSTANCEA for an `EnumDevices`
/// callback.
fn di_EnumDevices(c: &mut Cpu) {
    let m = c.mem();
    let (kind, cb, ctx) = (c.arg(1), c.arg(2), c.arg(3));
    let devices: &[([u8; 16], u32, &[u8])] = &[
        (GUID_SYSKEYBOARD, 0x0403, b"Keyboard\0"),
        (GUID_SYSMOUSE, 0x0202, b"Mouse\0"),
        (GUID_PAD, PAD_TYPE, b"Gamepad\0"),
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
        JOYSTICK => {
            // DIJOYSTATE(2): the axes (x, y, z, rx, ry, rz), two sliders, four POVs (centred),
            // the buttons; DIJOYSTATE2's velocities and forces stay zero.
            for i in (0..size).step_by(4) {
                m.st32(out + i, 0);
            }
            let (buttons, x, y) = s.pad;
            let axis = |v: i16| (i32::from(v) + 32768) as u32;
            m.st32(out, axis(x));
            m.st32(out + 4, axis(y));
            m.st32(out + 8, AXIS_MAX / 2);
            for k in 0..4 {
                m.st32(out + 32 + 4 * k, u32::MAX);
            }
            for b in 0..rc_platform::pad::BUTTONS {
                if buttons & (1 << b) != 0 && 48 + b < size {
                    m.st8(out + 48 + b, 0x80);
                }
            }
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

/// DIDEVCAPS: attached, the device's type, its axes, buttons and POVs; the joystick with force
/// feedback (the gamepads' vibration).
fn dev_GetCapabilities(c: &mut Cpu) {
    let m = c.mem();
    let (this, p) = (c.arg(0), c.arg(1));
    let size = m.ld32(p).clamp(24, 0x2c);
    for i in (4..size).step_by(4) {
        m.st32(p + i, 0);
    }
    let (kind, axes, buttons, povs) = match com::id_of(m, this) {
        KEYBOARD => (0x0403, 0, 128, 0),
        MOUSE => (0x0202, 3, 4, 0),
        _ => (PAD_TYPE, 2, rc_platform::pad::BUTTONS, 0),
    };
    // DIDC_ATTACHED, and for the joystick DIDC_FORCEFEEDBACK.
    let flags = if kind == PAD_TYPE { 0x101 } else { 1 };
    m.st32(p + 4, flags);
    m.st32(p + 8, kind);
    m.st32(p + 12, axes);
    m.st32(p + 16, buttons);
    m.st32(p + 20, povs);
    ret_stdcall(c, S_OK, 2);
}

/// The force feedback effect (the game makes one, a constant force it sets every frame): its
/// magnitude (0 to 10 000) and gain, whether it plays.
struct Effect {
    magnitude: u32,
    gain: u32,
    playing: bool,
}

static EFFECT: Mutex<Effect> = Mutex::new(Effect {
    magnitude: 0,
    gain: 10_000,
    playing: false,
});

/// The effect's strength to the gamepads.
fn rumble(e: &Effect) {
    let s = if e.playing {
        e.magnitude as f32 / 10_000.0 * e.gain as f32 / 10_000.0
    } else {
        0.0
    };
    static LAST: Mutex<f32> = Mutex::new(-1.0);
    let mut last = LAST.lock().expect("rumble");
    if (s - *last).abs() >= 0.01 {
        *last = s;
        if rc_runtime::host::env("RC_TRACE").is_some_and(|t| t.contains("rumble")) {
            rc_runtime::log!("[dinput] force feedback {s:.2}");
        }
    }
    rc_platform::rumble::set(s);
}

/// DIEFFECT's parameters the flags name: the gain (`DIEP_GAIN`), the constant force's magnitude
/// (`DIEP_TYPESPECIFICPARAMS`, a DICONSTANTFORCE); `DIEP_START` starts it.
fn effect_parameters(m: Mem, p: u32, flags: u32) {
    let mut e = EFFECT.lock().expect("effect");
    if p != 0 {
        if flags & 0x4 != 0 {
            e.gain = m.ld32(p + 0x10).min(10_000);
        }
        if flags & 0x100 != 0 && m.ld32(p + 0x2c) >= 4 && m.ld32(p + 0x30) != 0 {
            e.magnitude = (m.ld32(m.ld32(p + 0x30)) as i32).unsigned_abs().min(10_000);
        }
    }
    if flags & 0x2000_0000 != 0 {
        e.playing = true;
    }
    rumble(&e);
}

/// `CreateEffect(guid, DIEFFECT, out, outer)`: the one effect, with its parameters.
fn dev_CreateEffect(c: &mut Cpu) {
    let m = c.mem();
    let (p, out) = (c.arg(2), c.arg(3));
    effect_parameters(m, p, 0x104);
    m.st32(out, com::new_object(m, &EFFECT_IFACE, 0));
    ret_stdcall(c, S_OK, 5);
}

/// `SendForceFeedbackCommand`: a reset or stop (all) stops the vibration.
fn dev_SendForceFeedbackCommand(c: &mut Cpu) {
    let mut e = EFFECT.lock().expect("effect");
    e.playing = false;
    rumble(&e);
    drop(e);
    ret_stdcall(c, S_OK, 2);
}

fn effect_SetParameters(c: &mut Cpu) {
    let m = c.mem();
    effect_parameters(m, c.arg(1), c.arg(2));
    ret_stdcall(c, S_OK, 3);
}

fn effect_Start(c: &mut Cpu) {
    let mut e = EFFECT.lock().expect("effect");
    e.playing = true;
    rumble(&e);
    drop(e);
    ret_stdcall(c, S_OK, 3);
}

fn effect_Stop(c: &mut Cpu) {
    let mut e = EFFECT.lock().expect("effect");
    e.playing = false;
    rumble(&e);
    drop(e);
    ret_stdcall(c, S_OK, 1);
}

/// `DIPROP_RANGE` (4) of the joystick's axes; nothing else is asked.
fn dev_GetProperty(c: &mut Cpu) {
    let m = c.mem();
    let (prop, p) = (c.arg(1), c.arg(2));
    if prop == 4 {
        m.st32(p + 16, 0);
        m.st32(p + 20, AXIS_MAX);
        return ret_stdcall(c, S_OK, 3);
    }
    ret_stdcall(c, DIERR_UNSUPPORTED, 3)
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
    let kind = com::id_of(m, this);
    let name = if kind == KEYBOARD {
        key_name(obj)
    } else if kind == JOYSTICK {
        const NAMES: [&str; 12] = [
            "Square", "Triangle", "Cross", "Circle", "L2", "L1", "R2", "R1", "Select", "Start",
            "L3", "R3",
        ];
        match obj {
            0 => "X".into(),
            4 => "Y".into(),
            o => NAMES.get(o.wrapping_sub(48) as usize).map_or_else(
                || format!("Button {}", o.saturating_sub(47)),
                |n| (*n).into(),
            ),
        }
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
        ("GetCapabilities", Some(dev_GetCapabilities)),
        ("EnumObjects", None),
        ("GetProperty", Some(dev_GetProperty)),
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
        ("CreateEffect", Some(dev_CreateEffect)),
        ("EnumEffects", None),
        ("GetEffectInfo", None),
        ("GetForceFeedbackState", None),
        (
            "SendForceFeedbackCommand",
            Some(dev_SendForceFeedbackCommand),
        ),
        ("EnumCreatedEffectObjects", None),
        ("Escape", None),
        ("Poll", Some(ok1)),
        ("SendDeviceData", None),
    ],
};

static EFFECT_IFACE: Interface = Interface {
    dll: "dinput.dll",
    name: "IDirectInputEffect",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("Initialize", Some(ok_effect4)),
        ("GetEffectGuid", None),
        ("GetParameters", None),
        ("SetParameters", Some(effect_SetParameters)),
        ("Start", Some(effect_Start)),
        ("Stop", Some(effect_Stop)),
        ("GetEffectStatus", None),
        ("Download", Some(ok1)),
        ("Unload", Some(ok1)),
        ("Escape", None),
    ],
};

fn ok_effect4(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 4);
}

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

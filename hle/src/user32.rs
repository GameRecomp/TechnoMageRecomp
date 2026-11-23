// SPDX-License-Identifier: MIT
//! user32: the one game window, its message queue, the cursor. What TechnoMage imports (32
//! functions). Input reaches the queue from the platform layer (`post_input`).

#![allow(non_snake_case)]

use rc_runtime::cpu::Cpu;
use rc_runtime::dispatch::call_guest;
use rc_runtime::imports::{self, ret_stdcall};
use std::collections::VecDeque;
use std::sync::Mutex;

pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_ACTIVATE: u32 = 0x0006;
pub const WM_SETFOCUS: u32 = 0x0007;
pub const WM_PAINT: u32 = 0x000f;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_ACTIVATEAPP: u32 = 0x001c;
pub const WM_SETCURSOR: u32 = 0x0020;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;

const HWND: u32 = 0x0001_0010;
const HDC: u32 = 0x0002_0010;

#[derive(Debug, Clone, Copy)]
pub struct Msg {
    pub hwnd: u32,
    pub msg: u32,
    pub wparam: u32,
    pub lparam: u32,
}

struct State {
    /// Registered classes: (name, window procedure).
    classes: Vec<(String, u32)>,
    wndproc: u32,
    window: bool,
    width: u32,
    height: u32,
    queue: VecDeque<Msg>,
    cursor: (i32, i32),
    cursor_shown: i32,
    capture: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    classes: Vec::new(),
    wndproc: 0,
    window: false,
    width: 640,
    height: 480,
    queue: VecDeque::new(),
    cursor: (320, 240),
    cursor_shown: 0,
    capture: false,
});

fn st() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().expect("user32")
}

/// Input from the platform: queued for the game's message loop.
pub fn post_input(m: Msg) {
    let mut s = st();
    if m.msg == WM_MOUSEMOVE {
        s.cursor = (
            (m.lparam & 0xffff) as i16 as i32,
            (m.lparam >> 16) as i16 as i32,
        );
    }
    s.queue.push_back(m);
}

/// The game window's procedure, 0 before `CreateWindowExA`.
pub fn wndproc() -> u32 {
    st().wndproc
}

/// The size the game asked its window to be.
pub fn window_size() -> (u32, u32) {
    let s = st();
    (s.width, s.height)
}

fn send(c: &mut Cpu, msg: u32, wparam: u32, lparam: u32) -> u32 {
    let proc_ = st().wndproc;
    if proc_ == 0 {
        return 0;
    }
    call_guest(c, proc_, &[HWND, msg, wparam, lparam])
}

fn class_name(c: &Cpu, p: u32) -> String {
    if p < 0x10000 {
        format!("#{p}")
    } else {
        String::from_utf8_lossy(&c.mem().cstr(p, 256)).into_owned()
    }
}

fn RegisterClassA(c: &mut Cpu) {
    let m = c.mem();
    let wc = c.arg(0);
    let proc_ = m.ld32(wc + 4);
    let name = class_name(c, m.ld32(wc + 36));
    st().classes.push((name, proc_));
    ret_stdcall(c, 0xc001, 1);
}

fn UnregisterClassA(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

fn CreateWindowExA(c: &mut Cpu) {
    let name = class_name(c, c.arg(1));
    let (w, h) = (c.arg(6), c.arg(7));
    {
        let mut s = st();
        let proc_ = s
            .classes
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(&name))
            .map(|x| x.1)
            .unwrap_or(0);
        s.wndproc = proc_;
        s.window = true;
        if w > 0 && w < 0x8000 && h > 0 && h < 0x8000 {
            s.width = w;
            s.height = h;
        }
    }
    send(c, WM_CREATE, 0, 0);
    ret_stdcall(c, HWND, 12);
}

fn DestroyWindow(c: &mut Cpu) {
    send(c, WM_DESTROY, 0, 0);
    st().window = false;
    ret_stdcall(c, 1, 1);
}

fn DefWindowProcA(c: &mut Cpu) {
    let msg = c.arg(1);
    let r = match msg {
        WM_CLOSE => {
            send(c, WM_DESTROY, 0, 0);
            0
        }
        WM_SETCURSOR => 1,
        _ => 0,
    };
    ret_stdcall(c, r, 4);
}

fn PeekMessageA(c: &mut Cpu) {
    let m = c.mem();
    let (out, remove) = (c.arg(0), c.arg(4));
    crate::yield_point(c);
    let msg = {
        let mut s = st();
        if remove & 1 != 0 {
            s.queue.pop_front()
        } else {
            s.queue.front().copied()
        }
    };
    let r = match msg {
        Some(x) => {
            let (cx, cy) = st().cursor;
            let t = rc_runtime::clock::ticks_ms() as u32;
            for (i, v) in [x.hwnd, x.msg, x.wparam, x.lparam, t, cx as u32, cy as u32]
                .iter()
                .enumerate()
            {
                m.st32(out + 4 * i as u32, *v);
            }
            1
        }
        None => 0,
    };
    ret_stdcall(c, r, 5);
}

fn TranslateMessage(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

fn DispatchMessageA(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(0);
    let (msg, wparam, lparam) = (m.ld32(p + 4), m.ld32(p + 8), m.ld32(p + 12));
    let r = send(c, msg, wparam, lparam);
    ret_stdcall(c, r, 1);
}

fn SendMessageA(c: &mut Cpu) {
    let (msg, wparam, lparam) = (c.arg(1), c.arg(2), c.arg(3));
    let r = send(c, msg, wparam, lparam);
    ret_stdcall(c, r, 4);
}

fn PostMessageA(c: &mut Cpu) {
    let (hwnd, msg, wparam, lparam) = (c.arg(0), c.arg(1), c.arg(2), c.arg(3));
    st().queue.push_back(Msg {
        hwnd,
        msg,
        wparam,
        lparam,
    });
    ret_stdcall(c, 1, 4);
}

fn PostQuitMessage(c: &mut Cpu) {
    let code = c.arg(0);
    st().queue.push_back(Msg {
        hwnd: 0,
        msg: WM_QUIT,
        wparam: code,
        lparam: 0,
    });
    ret_stdcall(c, 0, 1);
}

fn MessageBoxA(c: &mut Cpu) {
    let m = c.mem();
    let text = crate::codepage::decode(&m.cstr(c.arg(1), 4096));
    let caption = crate::codepage::decode(&m.cstr(c.arg(2), 256));
    eprintln!("[user32] MessageBox \"{caption}\": {text}");
    ret_stdcall(c, 1, 4); // IDOK
}

fn GetSystemMetrics(c: &mut Cpu) {
    let r = match c.arg(0) {
        0 => 1024, // SM_CXSCREEN
        1 => 768,  // SM_CYSCREEN
        _ => 0,
    };
    ret_stdcall(c, r, 1);
}

fn AdjustWindowRect(c: &mut Cpu) {
    ret_stdcall(c, 1, 3);
}

fn SetWindowPos(c: &mut Cpu) {
    let (w, h, flags) = (c.arg(4), c.arg(5), c.arg(6));
    if flags & 1 == 0 && w > 0 && h > 0 && w < 0x8000 && h < 0x8000 {
        let mut s = st();
        s.width = w;
        s.height = h;
    }
    ret_stdcall(c, 1, 7);
}

fn UpdateWindow(c: &mut Cpu) {
    ret_stdcall(c, 1, 1);
}

fn SetFocus(c: &mut Cpu) {
    ret_stdcall(c, HWND, 1);
}

fn GetActiveWindow(c: &mut Cpu) {
    let w = if st().window { HWND } else { 0 };
    ret_stdcall(c, w, 0);
}

fn SetCapture(c: &mut Cpu) {
    st().capture = true;
    ret_stdcall(c, 0, 1);
}

fn ReleaseCapture(c: &mut Cpu) {
    st().capture = false;
    ret_stdcall(c, 1, 0);
}

fn ShowCursor(c: &mut Cpu) {
    let show = c.arg(0) != 0;
    let n = {
        let mut s = st();
        s.cursor_shown += if show { 1 } else { -1 };
        s.cursor_shown
    };
    ret_stdcall(c, n as u32, 1);
}

fn SetCursor(c: &mut Cpu) {
    ret_stdcall(c, 0x0003_0010, 1);
}

fn LoadCursorA(c: &mut Cpu) {
    ret_stdcall(c, 0x0003_0010, 2);
}

fn LoadIconA(c: &mut Cpu) {
    ret_stdcall(c, 0x0003_0020, 2);
}

fn GetCursorPos(c: &mut Cpu) {
    let p = c.arg(0);
    let (x, y) = st().cursor;
    let m = c.mem();
    m.st32(p, x as u32);
    m.st32(p + 4, y as u32);
    ret_stdcall(c, 1, 1);
}

fn SetCursorPos(c: &mut Cpu) {
    let (x, y) = (c.arg(0) as i32, c.arg(1) as i32);
    st().cursor = (x, y);
    ret_stdcall(c, 1, 2);
}

fn ClientToScreen(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

/// Virtual-key ↔ scan code for the keys a game asks about (US layout).
fn MapVirtualKeyA(c: &mut Cpu) {
    const SCAN: &[(u32, u32)] = &[
        (0x1b, 0x01),
        (0x08, 0x0e),
        (0x09, 0x0f),
        (0x0d, 0x1c),
        (0x11, 0x1d),
        (0x10, 0x2a),
        (0x12, 0x38),
        (0x20, 0x39),
        (0x25, 0x4b),
        (0x26, 0x48),
        (0x27, 0x4d),
        (0x28, 0x50),
        (0x70, 0x3b),
        (0x71, 0x3c),
        (0x72, 0x3d),
        (0x73, 0x3e),
        (0x74, 0x3f),
        (0x75, 0x40),
        (0x76, 0x41),
        (0x77, 0x42),
        (0x78, 0x43),
        (0x79, 0x44),
        (0x7a, 0x57),
        (0x7b, 0x58),
        (0x31, 0x02),
        (0x32, 0x03),
        (0x33, 0x04),
        (0x34, 0x05),
        (0x35, 0x06),
        (0x36, 0x07),
        (0x37, 0x08),
        (0x38, 0x09),
        (0x39, 0x0a),
        (0x30, 0x0b),
        (0x51, 0x10),
        (0x57, 0x11),
        (0x45, 0x12),
        (0x52, 0x13),
        (0x54, 0x14),
        (0x59, 0x15),
        (0x55, 0x16),
        (0x49, 0x17),
        (0x4f, 0x18),
        (0x50, 0x19),
        (0x41, 0x1e),
        (0x53, 0x1f),
        (0x44, 0x20),
        (0x46, 0x21),
        (0x47, 0x22),
        (0x48, 0x23),
        (0x4a, 0x24),
        (0x4b, 0x25),
        (0x4c, 0x26),
        (0x5a, 0x2c),
        (0x58, 0x2d),
        (0x43, 0x2e),
        (0x56, 0x2f),
        (0x42, 0x30),
        (0x4e, 0x31),
        (0x4d, 0x32),
    ];
    let (code, kind) = (c.arg(0), c.arg(1));
    let r = match kind {
        0 => SCAN.iter().find(|e| e.0 == code).map_or(0, |e| e.1),
        1 | 3 => SCAN.iter().find(|e| e.1 == code).map_or(0, |e| e.0),
        2 => match code {
            0x30..=0x39 | 0x41..=0x5a => code,
            0x20 => 0x20,
            _ => 0,
        },
        _ => 0,
    };
    ret_stdcall(c, r, 2);
}

fn GetDC(c: &mut Cpu) {
    ret_stdcall(c, HDC, 1);
}

fn ReleaseDC(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

fn BeginPaint(c: &mut Cpu) {
    let m = c.mem();
    let ps = c.arg(1);
    for i in 0..64 {
        m.st8(ps + i, 0);
    }
    m.st32(ps, HDC);
    ret_stdcall(c, HDC, 2);
}

fn EndPaint(c: &mut Cpu) {
    ret_stdcall(c, 1, 2);
}

macro_rules! table {
    ($($f:ident),* $(,)?) => {
        &[$((stringify!($f), $f as rc_runtime::dispatch::GuestFn)),*]
    };
}

pub fn register() {
    imports::register(
        "user32.dll",
        table![
            SetCapture,
            ShowCursor,
            PostQuitMessage,
            ReleaseCapture,
            DestroyWindow,
            SendMessageA,
            MessageBoxA,
            GetCursorPos,
            SetCursorPos,
            SetCursor,
            LoadCursorA,
            UpdateWindow,
            SetFocus,
            CreateWindowExA,
            RegisterClassA,
            LoadIconA,
            UnregisterClassA,
            ReleaseDC,
            GetDC,
            PostMessageA,
            SetWindowPos,
            AdjustWindowRect,
            GetSystemMetrics,
            DefWindowProcA,
            EndPaint,
            BeginPaint,
            DispatchMessageA,
            TranslateMessage,
            PeekMessageA,
            ClientToScreen,
            MapVirtualKeyA,
            GetActiveWindow,
        ],
    );
}

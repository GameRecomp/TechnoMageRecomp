// SPDX-License-Identifier: MIT
//! Guest threads, cooperative: one host thread per guest thread, and a baton so that exactly one
//! of them runs at a time. A thread gives way at the places the HLE layer calls [`yield_now`]
//! (`Sleep`, waits, the message loop); the next runnable thread in creation order runs until it
//! gives way in turn. As the sister projects' pthread backend.

use crate::cpu::{Cpu, ESP};
use crate::dispatch::call_guest;
use crate::mem::Mem;
use crate::vm;
use std::cell::Cell;
use std::sync::{Condvar, Mutex};

struct Sched {
    /// The thread holding the baton.
    current: u32,
    /// Live threads in creation order.
    live: Vec<u32>,
    /// Threads that have ended (for `WaitForSingleObject` on their handles), with exit codes.
    ended: Vec<(u32, u32)>,
    next: u32,
}

static SCHED: Mutex<Sched> = Mutex::new(Sched {
    current: 1,
    live: Vec::new(),
    ended: Vec::new(),
    next: 2,
});
static BATON: Condvar = Condvar::new();

thread_local! {
    static ME: Cell<u32> = const { Cell::new(1) };
}

/// The calling guest thread's id (1 is the main thread).
pub fn current() -> u32 {
    ME.with(|m| m.get())
}

/// Register the main thread (id 1) before anything else runs.
pub fn init_main() {
    let mut s = SCHED.lock().expect("sched");
    if s.live.is_empty() {
        s.live.push(1);
        s.current = 1;
    }
}

fn wait_turn(id: u32) {
    let mut s = SCHED.lock().expect("sched");
    while s.current != id {
        s = BATON.wait(s).expect("sched");
    }
}

/// Hand the baton to the next live thread and wait until it comes back. Returns at once when
/// this is the only thread.
pub fn yield_now() {
    let me = current();
    {
        let mut s = SCHED.lock().expect("sched");
        if s.live.len() <= 1 {
            return;
        }
        let i = s.live.iter().position(|&t| t == me).unwrap_or(0);
        s.current = s.live[(i + 1) % s.live.len()];
        BATON.notify_all();
        while s.current != me {
            s = BATON.wait(s).expect("sched");
        }
    }
}

/// Whether thread `id` has ended, and its exit code.
pub fn exit_code(id: u32) -> Option<u32> {
    SCHED
        .lock()
        .expect("sched")
        .ended
        .iter()
        .find(|e| e.0 == id)
        .map(|e| e.1)
}

/// Where the stacks and thread information blocks of new threads go.
const TIB_STRIDE: u32 = 0x1000;
const TIB_AREA: u32 = 0x7ff0_0000;

/// Start a guest thread at `entry` with `param` on a stack of `stack` bytes (1 MiB if 0). It
/// runs when the creating thread gives way. Returns its id.
pub fn spawn(m: Mem, entry: u32, param: u32, stack: u32) -> u32 {
    let size = if stack == 0 {
        0x0010_0000
    } else {
        stack.max(0x1_0000)
    };
    let lo = vm::region_alloc(0, size);
    let id = {
        let mut s = SCHED.lock().expect("sched");
        let id = s.next;
        s.next += 1;
        s.live.push(id);
        id
    };
    let tib = TIB_AREA + TIB_STRIDE * id;
    m.st32(tib, 0xffff_ffff);
    m.st32(tib + 0x04, lo + size);
    m.st32(tib + 0x08, lo);
    m.st32(tib + 0x18, tib);
    m.st32(tib + 0x20, 1);
    m.st32(tib + 0x24, id);
    m.st32(tib + 0x30, crate::process::PEB_ADDR);
    std::thread::Builder::new()
        .name(format!("guest-{id}"))
        .stack_size(16 << 20)
        .spawn(move || {
            ME.with(|me| me.set(id));
            wait_turn(id);
            let mut c = Cpu::new(m);
            c.fs_base = tib;
            c.r[ESP] = lo + size - 16;
            let code = call_guest(&mut c, entry, &[param]);
            finish(id, code);
        })
        .expect("a host thread for a guest thread");
    id
}

/// End the calling thread's turn for good (`ExitThread`, or its start function returned).
pub fn finish(id: u32, code: u32) {
    let mut s = SCHED.lock().expect("sched");
    let i = s.live.iter().position(|&t| t == id).unwrap_or(0);
    s.live.retain(|&t| t != id);
    s.ended.push((id, code));
    if !s.live.is_empty() {
        s.current = s.live[i % s.live.len()];
    }
    BATON.notify_all();
}

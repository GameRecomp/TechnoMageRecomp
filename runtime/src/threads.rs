// SPDX-License-Identifier: MIT
//! Guest threads, cooperative: exactly one runs at a time. A thread gives way at the places the
//! HLE layer calls [`yield_now`] (`Sleep`, waits, the message loop); the next runnable thread in
//! creation order runs until it gives way in turn.
//!
//! Natively each guest thread has a host thread, and a baton passes between them (as the sister
//! projects' pthread backend). In a browser each has a stack of the WebAssembly stack switching
//! proposal (JSPI) and a shadow stack of its own: the player's JavaScript starts a thread with
//! `rc_thread_main` and switches by resolving the promise the other one waits on
//! (web/player.js, `thread_new`, `thread_switch`, `thread_end`).

use crate::cpu::{Cpu, ESP};
use crate::dispatch::call_guest;
use crate::mem::Mem;
use crate::vm;
#[cfg(not(target_family = "wasm"))]
use std::cell::Cell;
#[cfg(not(target_family = "wasm"))]
use std::sync::Condvar;
use std::sync::Mutex;

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
#[cfg(not(target_family = "wasm"))]
static BATON: Condvar = Condvar::new();

#[cfg(not(target_family = "wasm"))]
thread_local! {
    static ME: Cell<u32> = const { Cell::new(1) };
}

/// The calling guest thread's id (1 is the main thread).
#[cfg(not(target_family = "wasm"))]
pub fn current() -> u32 {
    ME.with(|m| m.get())
}

/// The calling guest thread's id (1 is the main thread): the one holding the baton, as only it
/// runs.
#[cfg(target_family = "wasm")]
pub fn current() -> u32 {
    SCHED.lock().expect("sched").current
}

#[cfg(target_family = "wasm")]
mod web {
    #[link(wasm_import_module = "rc_js")]
    unsafe extern "C" {
        /// A new guest thread `id` whose shadow stack starts at `stack_top`; it runs when
        /// switched to.
        pub fn thread_new(id: u32, stack_top: u32);
        /// Run thread `to`, and return when this one is switched to again (suspends).
        pub fn thread_switch(to: u32);
        /// This thread has ended: run thread `to` (suspends for good).
        pub fn thread_end(to: u32);
    }
}

/// What a new thread runs: memory, entry, parameter, stack top, TIB.
#[cfg(target_family = "wasm")]
type Start = (Mem, u32, u32, u32, u32);

/// The threads that have not run yet (`rc_thread_main`).
#[cfg(target_family = "wasm")]
static STARTS: Mutex<Vec<(u32, Start)>> = Mutex::new(Vec::new());

/// The body of guest thread `id` in a browser: the player calls it (as a JSPI `promising`
/// export) when the thread first runs.
#[cfg(target_family = "wasm")]
pub fn thread_main(id: u32) {
    let start = {
        let mut st = STARTS.lock().expect("starts");
        let i = st.iter().position(|s| s.0 == id);
        i.map(|i| st.remove(i).1)
    };
    let Some((m, entry, param, top, tib)) = start else {
        return;
    };
    let mut c = Cpu::new(m);
    c.fs_base = tib;
    c.r[ESP] = top - 16;
    let code = call_guest(&mut c, entry, &[param]);
    end_current(id, code);
}

/// Register the main thread (id 1) before anything else runs.
pub fn init_main() {
    let mut s = SCHED.lock().expect("sched");
    if s.live.is_empty() {
        s.live.push(1);
        s.current = 1;
    }
}

#[cfg(not(target_family = "wasm"))]
fn wait_turn(id: u32) {
    let mut s = SCHED.lock().expect("sched");
    while s.current != id {
        s = BATON.wait(s).expect("sched");
    }
}

/// Hand the baton to the next live thread and wait until it comes back. Returns at once when
/// this is the only thread.
#[cfg(not(target_family = "wasm"))]
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

/// Hand the baton to the next live thread and wait until it comes back. Returns at once when
/// this is the only thread.
#[cfg(target_family = "wasm")]
pub fn yield_now() {
    let next = {
        let mut s = SCHED.lock().expect("sched");
        if s.live.len() <= 1 {
            return;
        }
        let me = s.current;
        let i = s.live.iter().position(|&t| t == me).unwrap_or(0);
        s.current = s.live[(i + 1) % s.live.len()];
        s.current
    };
    // SAFETY: a plain call; the player resumes this stack when the baton comes back.
    unsafe { web::thread_switch(next) };
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
const TIB_AREA: u32 = rc_abi::mem::GUEST_TOP - 0x10_0000;

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
    start(m, id, entry, param, lo + size, tib);
    id
}

#[cfg(not(target_family = "wasm"))]
fn start(m: Mem, id: u32, entry: u32, param: u32, top: u32, tib: u32) {
    std::thread::Builder::new()
        .name(format!("guest-{id}"))
        .stack_size(16 << 20)
        .spawn(move || {
            ME.with(|me| me.set(id));
            wait_turn(id);
            let mut c = Cpu::new(m);
            c.fs_base = tib;
            c.r[ESP] = top - 16;
            let code = call_guest(&mut c, entry, &[param]);
            finish(id, code);
        })
        .expect("a host thread for a guest thread");
}

#[cfg(target_family = "wasm")]
fn start(m: Mem, id: u32, entry: u32, param: u32, top: u32, tib: u32) {
    // The host side's (Rust's) stack for this thread, for the rest of the process.
    let shadow: &'static mut [u8] = Box::leak(vec![0u8; 1 << 20].into_boxed_slice());
    let shadow_top = (shadow.as_ptr() as usize + shadow.len()) as u32 & !15;
    STARTS
        .lock()
        .expect("starts")
        .push((id, (m, entry, param, top, tib)));
    // SAFETY: a plain call; the player keeps the id and the stack's address.
    unsafe { web::thread_new(id, shadow_top) };
}

/// End the calling guest thread (`ExitThread`, or its start function returned) and run the
/// next: never returns.
pub fn end_current(id: u32, code: u32) -> ! {
    finish(id, code);
    #[cfg(target_family = "wasm")]
    {
        let next = SCHED.lock().expect("sched").current;
        // SAFETY: a plain call; the player never resumes this stack.
        unsafe { web::thread_end(next) };
        unreachable!("an ended guest thread ran again");
    }
    #[cfg(not(target_family = "wasm"))]
    loop {
        // The host thread of an ended guest thread never runs guest code again.
        std::thread::park();
    }
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
    #[cfg(not(target_family = "wasm"))]
    BATON.notify_all();
}

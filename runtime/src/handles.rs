// SPDX-License-Identifier: MIT
//! Kernel object handles the HLE layer gives the guest: files, searches, synchronisation
//! objects, modules. Handle values are small multiples of 4 above `HANDLE_BASE`.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Mutex;

pub const HANDLE_BASE: u32 = 0x0000_0100;
pub const INVALID: u32 = 0xffff_ffff;
pub const STD_INPUT: u32 = 0x0000_0010;
pub const STD_OUTPUT: u32 = 0x0000_0014;
pub const STD_ERROR: u32 = 0x0000_0018;

#[derive(Debug)]
pub enum Object {
    File {
        file: File,
        path: PathBuf,
    },
    /// A `FindFirstFile` search: the matches still to hand out.
    Find(Vec<PathBuf>),
    Mutex {
        owned: u32,
    },
    Semaphore {
        count: i32,
        max: i32,
    },
    Event {
        set: bool,
        manual: bool,
    },
    Thread(u32),
    Process,
    Console(u32),
}

struct Table {
    next: u32,
    objects: BTreeMap<u32, Object>,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    next: HANDLE_BASE,
    objects: BTreeMap::new(),
});

pub fn insert(o: Object) -> u32 {
    let mut t = TABLE.lock().expect("handles");
    let h = t.next;
    t.next += 4;
    t.objects.insert(h, o);
    h
}

pub fn close(h: u32) -> bool {
    TABLE.lock().expect("handles").objects.remove(&h).is_some()
}

/// Run `f` on the object behind `h`.
pub fn with<R>(h: u32, f: impl FnOnce(&mut Object) -> R) -> Option<R> {
    let mut t = TABLE.lock().expect("handles");
    t.objects.get_mut(&h).map(f)
}

pub fn exists(h: u32) -> bool {
    TABLE.lock().expect("handles").objects.contains_key(&h)
}

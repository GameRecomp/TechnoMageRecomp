// SPDX-License-Identifier: MIT
//! dsound: DirectSound as Bink uses it for the videos' sound (the object comes from Miles'
//! `AIL_get_DirectSound_info`, or `DirectSoundCreate`). A secondary buffer's bytes live in guest
//! memory for `Lock`; `Unlock` copies what was written into the mixer's streaming copy, which
//! plays round and round; the play cursor is the mixer's position.

#![allow(non_snake_case)]

use crate::com::{self, Interface, S_OK};
use rc_runtime::cpu::Cpu;
use rc_runtime::imports::{self, ret_stdcall};
use rc_runtime::vm;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const VOICE_BASE: u32 = 0xd500_0000;

struct Buffer {
    /// Guest bytes (what `Lock` hands out).
    buf: u32,
    size: u32,
    channels: u16,
    bits: u16,
    rate: u32,
    block: u32,
    stream: Arc<Mutex<Vec<i16>>>,
    playing: bool,
    volume: f32,
}

static BUFFERS: Mutex<BTreeMap<u32, Buffer>> = Mutex::new(BTreeMap::new());
static NEXT: Mutex<u32> = Mutex::new(1);

fn ok1(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 1);
}
fn ok2(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 2);
}
fn ok3(c: &mut Cpu) {
    ret_stdcall(c, S_OK, 3);
}

/// The DirectSound object (one per process).
pub fn object(m: rc_runtime::mem::Mem) -> u32 {
    static OBJ: Mutex<u32> = Mutex::new(0);
    let mut o = OBJ.lock().expect("dsound");
    if *o == 0 {
        *o = com::new_object(m, &DSOUND, 0);
    }
    *o
}

fn DirectSoundCreate(c: &mut Cpu) {
    let m = c.mem();
    m.st32(c.arg(1), object(m));
    ret_stdcall(c, S_OK, 3);
}

fn ds_CreateSoundBuffer(c: &mut Cpu) {
    let m = c.mem();
    let (desc, out) = (c.arg(1), c.arg(2));
    let (flags, bytes, wfx) = (m.ld32(desc + 4), m.ld32(desc + 8), m.ld32(desc + 16));
    let (channels, rate, block, bits) = if wfx != 0 {
        (
            m.ld16(wfx + 2),
            m.ld32(wfx + 4),
            u32::from(m.ld16(wfx + 12)),
            m.ld16(wfx + 14),
        )
    } else {
        (2, 22050, 4, 16)
    };
    let primary = flags & 1 != 0;
    let size = if primary {
        4096
    } else {
        bytes.clamp(4, 16 << 20)
    };
    let id = {
        let mut n = NEXT.lock().expect("dsound");
        let id = *n;
        *n += 1;
        id
    };
    let buf = vm::heap_alloc(m, size, true);
    let frames = (size / u32::from(block.max(1) as u16).max(1)) as usize;
    BUFFERS.lock().expect("dsound").insert(
        id,
        Buffer {
            buf,
            size,
            channels: channels.max(1),
            bits,
            rate,
            block: block.max(1),
            stream: Arc::new(Mutex::new(vec![0; frames * usize::from(channels.max(1))])),
            playing: false,
            volume: 1.0,
        },
    );
    m.st32(out, com::new_object(m, &BUFFER, id));
    ret_stdcall(c, S_OK, 4);
}

fn ds_GetCaps(c: &mut Cpu) {
    let m = c.mem();
    let p = c.arg(1);
    let size = m.ld32(p).clamp(4, 96);
    for i in (4..size).step_by(4) {
        m.st32(p + i, 0);
    }
    m.st32(p + 4, 0x0f0f); // mono/stereo, 8/16-bit primary and secondary buffers
    m.st32(p + 8, 4000);
    m.st32(p + 12, 96_000);
    ret_stdcall(c, S_OK, 2);
}

fn ds_GetSpeakerConfig(c: &mut Cpu) {
    c.mem().st32(c.arg(1), 4); // stereo
    ret_stdcall(c, S_OK, 2);
}

fn bid(c: &Cpu) -> u32 {
    com::id_of(c.mem(), c.arg(0))
}

fn b_GetCurrentPosition(c: &mut Cpu) {
    let m = c.mem();
    let id = bid(c);
    let (play_p, write_p) = (c.arg(1), c.arg(2));
    let (size, block) = BUFFERS
        .lock()
        .expect("dsound")
        .get(&id)
        .map_or((4, 4), |b| (b.size, b.block));
    let pos = crate::mixer::position(VOICE_BASE + id).unwrap_or(0.0);
    let play = ((pos as u64 * u64::from(block)) % u64::from(size)) as u32;
    if play_p != 0 {
        m.st32(play_p, play);
    }
    if write_p != 0 {
        m.st32(write_p, (play + 4 * block) % size);
    }
    ret_stdcall(c, S_OK, 3);
}

fn b_GetFormat(c: &mut Cpu) {
    let m = c.mem();
    let id = bid(c);
    let (wfx, written) = (c.arg(1), c.arg(3));
    if let Some(b) = BUFFERS.lock().expect("dsound").get(&id) {
        if wfx != 0 {
            m.st16(wfx, 1);
            m.st16(wfx + 2, b.channels);
            m.st32(wfx + 4, b.rate);
            m.st32(wfx + 8, b.rate * b.block);
            m.st16(wfx + 12, b.block as u16);
            m.st16(wfx + 14, b.bits);
            m.st16(wfx + 16, 0);
        }
    }
    if written != 0 {
        m.st32(written, 18);
    }
    ret_stdcall(c, S_OK, 4);
}

fn b_GetStatus(c: &mut Cpu) {
    let id = bid(c);
    let playing = BUFFERS
        .lock()
        .expect("dsound")
        .get(&id)
        .is_some_and(|b| b.playing);
    c.mem().st32(c.arg(1), if playing { 1 | 4 } else { 0 }); // PLAYING | LOOPING
    ret_stdcall(c, S_OK, 2);
}

fn b_Lock(c: &mut Cpu) {
    let m = c.mem();
    let id = bid(c);
    let (off, bytes, p1, n1, p2, n2, flags) = (
        c.arg(1),
        c.arg(2),
        c.arg(3),
        c.arg(4),
        c.arg(5),
        c.arg(6),
        c.arg(7),
    );
    let Some((buf, size)) = BUFFERS
        .lock()
        .expect("dsound")
        .get(&id)
        .map(|b| (b.buf, b.size))
    else {
        return ret_stdcall(c, 0x8878_0032, 8); // DSERR_INVALIDPARAM
    };
    let (off, bytes) = if flags & 2 != 0 {
        (0, size)
    } else {
        (off % size, bytes.min(size))
    };
    let first = bytes.min(size - off);
    m.st32(p1, buf + off);
    m.st32(n1, first);
    if p2 != 0 {
        m.st32(p2, if bytes > first { buf } else { 0 });
    }
    if n2 != 0 {
        m.st32(n2, bytes - first);
    }
    ret_stdcall(c, S_OK, 8);
}

/// Copy the bytes written between `Lock` and `Unlock` into the mixer's copy.
fn copy_in(m: rc_runtime::mem::Mem, b: &Buffer, p: u32, n: u32) {
    if p == 0 || n == 0 || p < b.buf || p >= b.buf + b.size {
        return;
    }
    let mut stream = b.stream.lock().expect("stream");
    let mut raw = vec![0u8; n as usize];
    m.read(p, &mut raw);
    let start = p - b.buf;
    if b.bits == 8 {
        for (i, &x) in raw.iter().enumerate() {
            if let Some(s) = stream.get_mut(start as usize + i) {
                *s = (i16::from(x) - 128) << 8;
            }
        }
    } else {
        for (i, x) in raw.as_chunks::<2>().0.iter().enumerate() {
            if let Some(s) = stream.get_mut(start as usize / 2 + i) {
                *s = i16::from_le_bytes(*x);
            }
        }
    }
}

fn b_Unlock(c: &mut Cpu) {
    let m = c.mem();
    let id = bid(c);
    let (p1, n1, p2, n2) = (c.arg(1), c.arg(2), c.arg(3), c.arg(4));
    if let Some(b) = BUFFERS.lock().expect("dsound").get(&id) {
        copy_in(m, b, p1, n1);
        copy_in(m, b, p2, n2);
    }
    ret_stdcall(c, S_OK, 5);
}

fn b_Play(c: &mut Cpu) {
    let id = bid(c);
    let v = {
        let mut bs = BUFFERS.lock().expect("dsound");
        bs.get_mut(&id).map(|b| {
            b.playing = true;
            (b.stream.clone(), b.channels, b.rate, b.volume)
        })
    };
    if let Some((s, ch, rate, vol)) = v {
        crate::mixer::play_stream(VOICE_BASE + id, s, ch, rate, vol, 0.0);
    }
    ret_stdcall(c, S_OK, 4);
}

fn b_Stop(c: &mut Cpu) {
    let id = bid(c);
    if let Some(b) = BUFFERS.lock().expect("dsound").get_mut(&id) {
        b.playing = false;
    }
    crate::mixer::stop(VOICE_BASE + id);
    ret_stdcall(c, S_OK, 1);
}

fn b_SetVolume(c: &mut Cpu) {
    let id = bid(c);
    // Hundredths of a decibel, 0 full, -10000 silent.
    let db = (c.arg(1) as i32).clamp(-10000, 0) as f32 / 100.0;
    let v = 10f32.powf(db / 20.0);
    if let Some(b) = BUFFERS.lock().expect("dsound").get_mut(&id) {
        b.volume = v;
    }
    crate::mixer::set(VOICE_BASE + id, |x| x.volume = v);
    ret_stdcall(c, S_OK, 2);
}

fn b_SetFrequency(c: &mut Cpu) {
    let (id, f) = (bid(c), c.arg(1));
    if f != 0 {
        crate::mixer::set(VOICE_BASE + id, |x| x.rate = f);
    }
    ret_stdcall(c, S_OK, 2);
}

fn b_GetVolume(c: &mut Cpu) {
    c.mem().st32(c.arg(1), 0);
    ret_stdcall(c, S_OK, 2);
}

static DSOUND: Interface = Interface {
    dll: "dsound.dll",
    name: "IDirectSound",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("CreateSoundBuffer", Some(ds_CreateSoundBuffer)),
        ("GetCaps", Some(ds_GetCaps)),
        ("DuplicateSoundBuffer", None),
        ("SetCooperativeLevel", Some(ok3)),
        ("Compact", Some(ok1)),
        ("GetSpeakerConfig", Some(ds_GetSpeakerConfig)),
        ("SetSpeakerConfig", Some(ok2)),
        ("Initialize", Some(ok2)),
    ],
};

static BUFFER: Interface = Interface {
    dll: "dsound.dll",
    name: "IDirectSoundBuffer",
    methods: &[
        ("QueryInterface", None),
        ("AddRef", Some(com::add_ref)),
        ("Release", Some(com::release)),
        ("GetCaps", None),
        ("GetCurrentPosition", Some(b_GetCurrentPosition)),
        ("GetFormat", Some(b_GetFormat)),
        ("GetVolume", Some(b_GetVolume)),
        ("GetPan", Some(b_GetVolume)),
        ("GetFrequency", None),
        ("GetStatus", Some(b_GetStatus)),
        ("Initialize", Some(ok3)),
        ("Lock", Some(b_Lock)),
        ("Play", Some(b_Play)),
        ("SetCurrentPosition", Some(ok2)),
        ("SetFormat", Some(ok2)),
        ("SetVolume", Some(b_SetVolume)),
        ("SetPan", Some(ok2)),
        ("SetFrequency", Some(b_SetFrequency)),
        ("Stop", Some(b_Stop)),
        ("Unlock", Some(b_Unlock)),
        ("Restore", Some(ok1)),
    ],
};

pub fn register() {
    imports::register(
        "dsound.dll",
        &[(
            "DirectSoundCreate",
            DirectSoundCreate as rc_runtime::dispatch::GuestFn,
        )],
    );
}

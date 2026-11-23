// SPDX-License-Identifier: MIT
//! mss32: the Miles Sound System 6 functions TechnoMage imports (45): the digital driver, 2D
//! samples and 3D samples with one provider ("DirectSound3D 7+ Software - Pan and Volume", the
//! one `TechnoMage.ini` names). This first version keeps the state the game sets and reads back;
//! a sample "plays" for the length of its data at its rate, so the game's waits end. The mixer
//! that makes it audible comes with the platform's audio (docs/PLAN.md, M6).

#![allow(non_snake_case)]

use rc_runtime::cpu::Cpu;
use rc_runtime::imports::{self, ret_stdcall};
use rc_runtime::mem::Mem;
use rc_runtime::{clock, vm};
use std::collections::BTreeMap;
use std::sync::Mutex;

const SMP_FREE: u32 = 1;
const SMP_DONE: u32 = 2;
const SMP_PLAYING: u32 = 4;

const PROVIDER: u32 = 0x0000_4d33; // "M3"
const LISTENER: u32 = 0x0000_4c31;
const PROVIDER_NAME: &[u8] = b"DirectSound3D 7+ Software - Pan and Volume\0";

#[derive(Debug, Default, Clone)]
struct Sample {
    /// The decoded sound, once the game handed it over.
    pcm: Option<std::sync::Arc<crate::mixer::Pcm>>,
    /// Format of raw data (`AIL_set_sample_type`): DIG_F_* (0 mono 8, 1 mono 16, 2 stereo 8,
    /// 3 stereo 16).
    raw_format: u32,
    /// 3D distances: (maximum, minimum).
    distances: (f32, f32),
    /// A 3D sample (positioned in the listener's space by the mixer, M6).
    #[allow(dead_code)]
    three_d: bool,
    status: u32,
    /// Address and length of the data the game handed over.
    data: u32,
    len: u32,
    rate: u32,
    loops: u32,
    volume: u32,
    pan: u32,
    position: [f32; 3],
    started_ms: u64,
}

/// The listener: position, facing and up vectors.
static LISTENER_POS: Mutex<([f32; 3], [f32; 3], [f32; 3])> =
    Mutex::new(([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]));

impl Sample {
    fn status(&mut self, h: u32) -> u32 {
        if self.status == SMP_PLAYING && !crate::mixer::playing(h) {
            self.status = SMP_DONE;
        }
        self.status
    }

    /// Gain and pan: the 2D settings, and for a 3D sample its place relative to the listener.
    fn gain_pan(&self) -> (f32, f32) {
        let volume = (self.volume.min(127) as f32 / 127.0).powf(1.5);
        if !self.three_d {
            return (volume, (self.pan.min(127) as f32 - 64.0) / 64.0);
        }
        let (lp, face, up) = *LISTENER_POS.lock().expect("listener");
        let d = [
            self.position[0] - lp[0],
            self.position[1] - lp[1],
            self.position[2] - lp[2],
        ];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let (max, min) = self.distances;
        let min = if min > 0.0 { min } else { 1.0 };
        let mut g = if dist <= min { 1.0 } else { min / dist };
        if max > 0.0 && dist > max {
            g = 0.0;
        }
        // Right = up × facing.
        let right = [
            up[1] * face[2] - up[2] * face[1],
            up[2] * face[0] - up[0] * face[2],
            up[0] * face[1] - up[1] * face[0],
        ];
        let rl = (right[0] * right[0] + right[1] * right[1] + right[2] * right[2]).sqrt();
        let pan = if dist > 1e-3 && rl > 1e-6 {
            (d[0] * right[0] + d[1] * right[1] + d[2] * right[2]) / (dist * rl)
        } else {
            0.0
        };
        (volume * g, pan.clamp(-1.0, 1.0) * 0.8)
    }
}

/// Apply a sample's current gain and pan to its playing voice.
fn update_voice(h: u32) {
    let gp = st().samples.get(&h).map(|s| s.gain_pan());
    if let Some((g, p)) = gp {
        crate::mixer::set(h, |v| {
            v.volume = g;
            v.pan = p;
        });
    }
}

struct State {
    samples: BTreeMap<u32, Sample>,
    provider_name: u32,
    driver: u32,
}

static STATE: Mutex<State> = Mutex::new(State {
    samples: BTreeMap::new(),
    provider_name: 0,
    driver: 0,
});

fn st() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().expect("mss32")
}

/// A handle: a small block of guest memory, so it is a valid pointer if the game looks.
fn new_handle(m: Mem, three_d: bool) -> u32 {
    let h = vm::heap_alloc(m, 64, true);
    st().samples.insert(
        h,
        Sample {
            three_d,
            status: SMP_DONE,
            volume: 127,
            pan: 64,
            loops: 1,
            rate: 22050,
            ..Sample::default()
        },
    );
    h
}

fn with<R>(h: u32, f: impl FnOnce(&mut Sample) -> R) -> Option<R> {
    let r = st().samples.get_mut(&h).map(f);
    update_voice(h);
    r
}

/// The WAV data's length and rate, from a RIFF file in guest memory.
fn wav_info(m: Mem, a: u32) -> (u32, u32) {
    let mut hdr = [0u8; 44];
    m.read(a, &mut hdr);
    if &hdr[0..4] == b"RIFF" && &hdr[8..12] == b"WAVE" {
        let rate = u32::from_le_bytes([hdr[24], hdr[25], hdr[26], hdr[27]]);
        let size = u32::from_le_bytes([hdr[40], hdr[41], hdr[42], hdr[43]]);
        (size, rate)
    } else {
        (0, 22050)
    }
}

fn AIL_startup(c: &mut Cpu) {
    ret_stdcall(c, 1, 0);
}

fn AIL_shutdown(c: &mut Cpu) {
    ret_stdcall(c, 0, 0);
}

fn AIL_set_redist_directory(c: &mut Cpu) {
    let p = c.arg(0);
    ret_stdcall(c, p, 1);
}

fn AIL_set_preference(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn AIL_waveOutOpen(c: &mut Cpu) {
    let m = c.mem();
    let out = c.arg(0);
    let d = vm::heap_alloc(m, 256, true);
    st().driver = d;
    if out != 0 {
        m.st32(out, d);
    }
    if c.arg(1) != 0 {
        m.st32(c.arg(1), 0);
    }
    ret_stdcall(c, 0, 4);
}

fn AIL_waveOutClose(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

/// The DirectSound object Miles plays through: Bink plays the videos' sound through it.
fn AIL_get_DirectSound_info(c: &mut Cpu) {
    let m = c.mem();
    let (ds, buf) = (c.arg(1), c.arg(2));
    if ds != 0 {
        m.st32(ds, crate::dsound::object(m));
    }
    if buf != 0 {
        m.st32(buf, 0);
    }
    ret_stdcall(c, 0, 3);
}

fn AIL_allocate_sample_handle(c: &mut Cpu) {
    let h = new_handle(c.mem(), false);
    ret_stdcall(c, h, 1);
}

fn AIL_release_sample_handle(c: &mut Cpu) {
    let h = c.arg(0);
    st().samples.remove(&h);
    crate::mixer::remove(h);
    vm::heap_free(h);
    ret_stdcall(c, 0, 1);
}

fn AIL_init_sample(c: &mut Cpu) {
    let h = c.arg(0);
    with(h, |s| {
        s.status = SMP_DONE;
        s.loops = 1;
    });
    ret_stdcall(c, 0, 1);
}

fn AIL_set_sample_file(c: &mut Cpu) {
    let m = c.mem();
    let (h, p) = (c.arg(0), c.arg(1));
    let (len, rate) = wav_info(m, p);
    // The whole RIFF file: its size is in the header.
    let size = m.ld32(p + 4).saturating_add(8).min(64 << 20);
    let mut file = vec![0u8; size as usize];
    m.read(p, &mut file);
    let pcm = crate::mixer::decode_wav(&file).map(std::sync::Arc::new);
    let ok = with(h, |s| {
        s.data = p;
        s.len = len;
        s.rate = pcm.as_ref().map_or(rate, |x| x.rate);
        s.pcm = pcm;
        s.status = SMP_DONE;
    })
    .is_some();
    ret_stdcall(c, u32::from(ok), 3);
}

fn AIL_set_sample_address(c: &mut Cpu) {
    let m = c.mem();
    let (h, p, len) = (c.arg(0), c.arg(1), c.arg(2));
    let mut data = vec![0u8; len.min(64 << 20) as usize];
    m.read(p, &mut data);
    with(h, |s| {
        s.data = p;
        s.len = len;
        let (bits, channels) = match s.raw_format & 3 {
            0 => (8, 1),
            1 => (16, 1),
            2 => (8, 2),
            _ => (16, 2),
        };
        s.pcm = Some(std::sync::Arc::new(crate::mixer::Pcm {
            samples: crate::mixer::decode_raw(&data, 1, bits, channels, 0),
            channels,
            rate: s.rate,
        }));
    });
    ret_stdcall(c, 0, 3);
}

fn AIL_set_sample_type(c: &mut Cpu) {
    let (h, f) = (c.arg(0), c.arg(1));
    with(h, |s| s.raw_format = f);
    ret_stdcall(c, 0, 3);
}

fn AIL_set_sample_playback_rate(c: &mut Cpu) {
    let (h, r) = (c.arg(0), c.arg(1));
    with(h, |s| s.rate = r);
    ret_stdcall(c, 0, 2);
}

fn AIL_set_sample_volume(c: &mut Cpu) {
    let (h, v) = (c.arg(0), c.arg(1));
    with(h, |s| s.volume = v);
    ret_stdcall(c, 0, 2);
}

fn AIL_set_sample_pan(c: &mut Cpu) {
    let (h, v) = (c.arg(0), c.arg(1));
    with(h, |s| s.pan = v);
    ret_stdcall(c, 0, 2);
}

fn AIL_set_sample_loop_count(c: &mut Cpu) {
    let (h, n) = (c.arg(0), c.arg(1));
    with(h, |s| s.loops = n);
    ret_stdcall(c, 0, 2);
}

fn AIL_set_sample_position(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn AIL_sample_position(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

fn start(h: u32) {
    let voice = st().samples.get_mut(&h).and_then(|s| {
        s.status = SMP_PLAYING;
        s.started_ms = clock::ticks_ms();
        s.pcm.clone().map(|p| (p, s.rate, s.loops, s.gain_pan()))
    });
    match voice {
        Some((pcm, rate, loops, (g, p))) => crate::mixer::play(h, pcm, rate, loops, g, p),
        None => {
            with(h, |s| s.status = SMP_DONE);
        }
    }
}

fn AIL_resume_sample(c: &mut Cpu) {
    start(c.arg(0));
    ret_stdcall(c, 0, 1);
}

fn AIL_end_sample(c: &mut Cpu) {
    crate::mixer::stop(c.arg(0));
    with(c.arg(0), |s| s.status = SMP_DONE);
    ret_stdcall(c, 0, 1);
}

fn AIL_sample_status(c: &mut Cpu) {
    let h = c.arg(0);
    let r = st()
        .samples
        .get_mut(&h)
        .map(|s| s.status(h))
        .unwrap_or(SMP_FREE);
    ret_stdcall(c, r, 1);
}

// --- 3D ------------------------------------------------------------------------------------

fn AIL_enumerate_3D_providers(c: &mut Cpu) {
    let m = c.mem();
    let (next, dest, name) = (c.arg(0), c.arg(1), c.arg(2));
    let n = m.ld32(next);
    let r = if n == 0 {
        let p = {
            let mut s = st();
            if s.provider_name == 0 {
                drop(s);
                let a = vm::heap_alloc(m, PROVIDER_NAME.len() as u32, false);
                m.write(a, PROVIDER_NAME);
                s = st();
                s.provider_name = a;
            }
            s.provider_name
        };
        m.st32(next, 1);
        m.st32(dest, PROVIDER);
        m.st32(name, p);
        1
    } else {
        0
    };
    ret_stdcall(c, r, 3);
}

fn AIL_set_3D_provider_preference(c: &mut Cpu) {
    ret_stdcall(c, 0, 3);
}

fn AIL_open_3D_provider(c: &mut Cpu) {
    ret_stdcall(c, 0, 1); // M3D_NOERR
}

fn AIL_close_3D_provider(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

fn AIL_set_3D_speaker_type(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn AIL_set_3D_room_type(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn AIL_open_3D_listener(c: &mut Cpu) {
    ret_stdcall(c, LISTENER, 1);
}

fn AIL_close_3D_listener(c: &mut Cpu) {
    ret_stdcall(c, 0, 1);
}

fn AIL_allocate_3D_sample_handle(c: &mut Cpu) {
    let h = new_handle(c.mem(), true);
    ret_stdcall(c, h, 1);
}

fn AIL_release_3D_sample_handle(c: &mut Cpu) {
    let h = c.arg(0);
    st().samples.remove(&h);
    crate::mixer::remove(h);
    vm::heap_free(h);
    ret_stdcall(c, 0, 1);
}

fn AIL_set_3D_sample_info(c: &mut Cpu) {
    let m = c.mem();
    let (h, info) = (c.arg(0), c.arg(1));
    // AILSOUNDINFO: format, data_ptr, data_len, rate, bits, channels, samples, block, initial_ptr.
    let (tag, data, len, rate) = (
        m.ld32(info),
        m.ld32(info + 4),
        m.ld32(info + 8),
        m.ld32(info + 12),
    );
    let (bits, channels, block) = (m.ld32(info + 16), m.ld32(info + 20), m.ld32(info + 28));
    let mut raw = vec![0u8; len.min(64 << 20) as usize];
    m.read(data, &mut raw);
    let pcm = crate::mixer::Pcm {
        samples: crate::mixer::decode_raw(
            &raw,
            tag as u16,
            bits as u16,
            channels as u16,
            block as usize,
        ),
        channels: (channels as u16).max(1),
        rate,
    };
    let ok = with(h, |s| {
        s.data = data;
        s.len = len;
        s.rate = rate;
        s.pcm = Some(std::sync::Arc::new(pcm));
        s.status = SMP_DONE;
    })
    .is_some();
    ret_stdcall(c, u32::from(ok), 2);
}

fn AIL_set_3D_sample_loop_count(c: &mut Cpu) {
    let (h, n) = (c.arg(0), c.arg(1));
    with(h, |s| s.loops = n);
    ret_stdcall(c, 0, 2);
}

fn AIL_set_3D_sample_playback_rate(c: &mut Cpu) {
    let (h, r) = (c.arg(0), c.arg(1));
    with(h, |s| s.rate = r);
    ret_stdcall(c, 0, 2);
}

fn AIL_set_3D_sample_volume(c: &mut Cpu) {
    let (h, v) = (c.arg(0), c.arg(1));
    with(h, |s| s.volume = v);
    ret_stdcall(c, 0, 2);
}

fn AIL_set_3D_sample_effects_level(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn AIL_set_3D_sample_occlusion(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn AIL_set_3D_sample_obstruction(c: &mut Cpu) {
    ret_stdcall(c, 0, 2);
}

fn AIL_set_3D_sample_distances(c: &mut Cpu) {
    let (h, max, min) = (c.arg(0), f32::from_bits(c.arg(1)), f32::from_bits(c.arg(2)));
    with(h, |s| s.distances = (max, min));
    ret_stdcall(c, 0, 3);
}

fn AIL_set_3D_position(c: &mut Cpu) {
    let h = c.arg(0);
    let p = [
        f32::from_bits(c.arg(1)),
        f32::from_bits(c.arg(2)),
        f32::from_bits(c.arg(3)),
    ];
    if h == LISTENER {
        LISTENER_POS.lock().expect("listener").0 = p;
        let all: Vec<u32> = st().samples.keys().copied().collect();
        for s in all {
            update_voice(s);
        }
    } else {
        with(h, |s| s.position = p);
    }
    ret_stdcall(c, 0, 4);
}

fn AIL_set_3D_velocity_vector(c: &mut Cpu) {
    ret_stdcall(c, 0, 4);
}

fn AIL_set_3D_orientation(c: &mut Cpu) {
    let h = c.arg(0);
    let v: Vec<f32> = (1..7).map(|i| f32::from_bits(c.arg(i))).collect();
    if h == LISTENER {
        let mut l = LISTENER_POS.lock().expect("listener");
        l.1 = [v[0], v[1], v[2]];
        l.2 = [v[3], v[4], v[5]];
    }
    ret_stdcall(c, 0, 7);
}

fn AIL_resume_3D_sample(c: &mut Cpu) {
    start(c.arg(0));
    ret_stdcall(c, 0, 1);
}

fn AIL_end_3D_sample(c: &mut Cpu) {
    crate::mixer::stop(c.arg(0));
    with(c.arg(0), |s| s.status = SMP_DONE);
    ret_stdcall(c, 0, 1);
}

macro_rules! table {
    ($($f:ident = $n:expr),* $(,)?) => {
        &[$(($n, $f as rc_runtime::dispatch::GuestFn)),*]
    };
}

pub fn register() {
    imports::register(
        "mss32.dll",
        table![
            AIL_resume_3D_sample = "_AIL_resume_3D_sample@4",
            AIL_sample_position = "_AIL_sample_position@4",
            AIL_set_sample_file = "_AIL_set_sample_file@12",
            AIL_init_sample = "_AIL_init_sample@4",
            AIL_set_sample_type = "_AIL_set_sample_type@12",
            AIL_set_sample_address = "_AIL_set_sample_address@12",
            AIL_set_sample_loop_count = "_AIL_set_sample_loop_count@8",
            AIL_set_sample_position = "_AIL_set_sample_position@8",
            AIL_set_sample_playback_rate = "_AIL_set_sample_playback_rate@8",
            AIL_set_sample_pan = "_AIL_set_sample_pan@8",
            AIL_set_sample_volume = "_AIL_set_sample_volume@8",
            AIL_set_3D_sample_info = "_AIL_set_3D_sample_info@8",
            AIL_set_3D_sample_loop_count = "_AIL_set_3D_sample_loop_count@8",
            AIL_set_3D_sample_playback_rate = "_AIL_set_3D_sample_playback_rate@8",
            AIL_set_3D_sample_volume = "_AIL_set_3D_sample_volume@8",
            AIL_set_3D_sample_effects_level = "_AIL_set_3D_sample_effects_level@8",
            AIL_set_3D_sample_occlusion = "_AIL_set_3D_sample_occlusion@8",
            AIL_set_3D_sample_obstruction = "_AIL_set_3D_sample_obstruction@8",
            AIL_set_3D_velocity_vector = "_AIL_set_3D_velocity_vector@16",
            AIL_resume_sample = "_AIL_resume_sample@4",
            AIL_sample_status = "_AIL_sample_status@4",
            AIL_set_3D_position = "_AIL_set_3D_position@16",
            AIL_set_3D_room_type = "_AIL_set_3D_room_type@8",
            AIL_end_sample = "_AIL_end_sample@4",
            AIL_release_sample_handle = "_AIL_release_sample_handle@4",
            AIL_end_3D_sample = "_AIL_end_3D_sample@4",
            AIL_release_3D_sample_handle = "_AIL_release_3D_sample_handle@4",
            AIL_close_3D_listener = "_AIL_close_3D_listener@4",
            AIL_close_3D_provider = "_AIL_close_3D_provider@4",
            AIL_waveOutClose = "_AIL_waveOutClose@4",
            AIL_shutdown = "_AIL_shutdown@0",
            AIL_set_redist_directory = "_AIL_set_redist_directory@4",
            AIL_startup = "_AIL_startup@0",
            AIL_set_preference = "_AIL_set_preference@8",
            AIL_waveOutOpen = "_AIL_waveOutOpen@16",
            AIL_allocate_sample_handle = "_AIL_allocate_sample_handle@4",
            AIL_enumerate_3D_providers = "_AIL_enumerate_3D_providers@12",
            AIL_open_3D_provider = "_AIL_open_3D_provider@4",
            AIL_set_3D_speaker_type = "_AIL_set_3D_speaker_type@8",
            AIL_allocate_3D_sample_handle = "_AIL_allocate_3D_sample_handle@4",
            AIL_set_3D_sample_distances = "_AIL_set_3D_sample_distances@12",
            AIL_set_3D_orientation = "_AIL_set_3D_orientation@28",
            AIL_set_3D_provider_preference = "_AIL_set_3D_provider_preference@12",
            AIL_open_3D_listener = "_AIL_open_3D_listener@4",
            AIL_get_DirectSound_info = "_AIL_get_DirectSound_info@12",
        ],
    );
}

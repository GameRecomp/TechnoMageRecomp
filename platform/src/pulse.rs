// SPDX-License-Identifier: MIT
//! Sound through PulseAudio on Linux (`libpulse-simple`, loaded at run time). Bevy's audio
//! (cpal) speaks only ALSA on Linux, and on a desktop where PipeWire or PulseAudio holds the
//! sound card that needs ALSA's bridge to them (`pipewire-alsa`, `alsa-plugins-pulseaudio`),
//! which not every system has: the game stayed silent there. PipeWire serves PulseAudio's clients
//! too (`pipewire-pulse`), so this reaches nearly every desktop, as SDL does. Without the library,
//! or with `RC_AUDIO=alsa`, the sound goes through Bevy as before.

use std::ffi::{c_char, c_int, c_void};

/// `pa_sample_spec`.
#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

/// `pa_buffer_attr` (`u32::MAX`: the server's choice).
#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

/// `PA_SAMPLE_FLOAT32LE`, `PA_STREAM_PLAYBACK`.
const FLOAT32LE: c_int = 5;
const PLAYBACK: c_int = 1;

type New = unsafe extern "C" fn(
    server: *const c_char,
    name: *const c_char,
    dir: c_int,
    dev: *const c_char,
    stream_name: *const c_char,
    ss: *const SampleSpec,
    map: *const c_void,
    attr: *const BufferAttr,
    error: *mut c_int,
) -> *mut c_void;
type Write = unsafe extern "C" fn(
    s: *mut c_void,
    data: *const c_void,
    bytes: usize,
    error: *mut c_int,
) -> c_int;

/// Frames mixed at a time (about 12 ms).
const FRAMES: usize = 512;

/// Start playing the mixer (`crate::audio`) through PulseAudio; false when it cannot be had
/// (then Bevy plays it).
pub fn start(app_name: &str) -> bool {
    if std::env::var("RC_AUDIO").is_ok_and(|v| v == "alsa") {
        return false;
    }
    // SAFETY: loading a system library runs its initialisers; libpulse-simple's are plain.
    let lib = match unsafe { libloading::Library::new("libpulse-simple.so.0") } {
        Ok(l) => l,
        Err(_) => return false,
    };
    // SAFETY: the symbols have these C signatures (pulse/simple.h).
    let (new, write) = unsafe {
        match (
            lib.get::<New>(b"pa_simple_new\0"),
            lib.get::<Write>(b"pa_simple_write\0"),
        ) {
            (Ok(n), Ok(w)) => (*n, *w),
            _ => return false,
        }
    };
    let name = std::ffi::CString::new(app_name).unwrap_or_default();
    let stream = c"Game";
    let spec = SampleSpec {
        format: FLOAT32LE,
        rate: crate::audio::RATE,
        channels: 2,
    };
    // About 60 ms in the server's buffer: little delay, no gaps when a frame takes long.
    let bytes = |ms: u32| crate::audio::RATE * 2 * 4 * ms / 1000;
    let attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: bytes(60),
        prebuf: u32::MAX,
        minreq: bytes(10),
        fragsize: u32::MAX,
    };
    let mut error = 0;
    // SAFETY: the strings, the spec and the attributes are valid for the call; null server,
    // device and channel map ask for the defaults.
    let s = unsafe {
        new(
            std::ptr::null(),
            name.as_ptr(),
            PLAYBACK,
            std::ptr::null(),
            stream.as_ptr(),
            &spec,
            std::ptr::null(),
            &attr,
            &mut error,
        )
    };
    if s.is_null() {
        eprintln!("[audio] PulseAudio: no stream (error {error}); ALSA instead");
        return false;
    }
    let s = s as usize;
    let spawned = std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            // The library stays loaded while the thread plays.
            let _lib = lib;
            let mut buf = vec![0f32; 2 * FRAMES];
            loop {
                crate::audio::mix(&mut buf);
                let mut error = 0;
                // SAFETY: the stream lives as long as the program; the samples are valid for
                // the call. It blocks until the server has room, which paces the mixer.
                let r = unsafe {
                    write(
                        s as *mut c_void,
                        buf.as_ptr().cast(),
                        buf.len() * 4,
                        &mut error,
                    )
                };
                if r < 0 {
                    eprintln!("[audio] PulseAudio: writing failed (error {error})");
                    return;
                }
            }
        });
    if spawned.is_err() {
        return false;
    }
    eprintln!("[audio] PulseAudio");
    true
}

// SPDX-License-Identifier: MIT
//! The audio mixer behind Miles and DirectSound: voices of decoded PCM on the host (copied out
//! of guest memory when the game hands it over, so the audio thread never reads guest memory),
//! mixed to stereo at the platform's rate with volume, pan, loops and resampling.
//!
//! Without an audio device (headless) nothing pulls the mixer; voices then end by the clock, so
//! the game's waits for a sound still end.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Decoded audio: interleaved 16-bit samples.
#[derive(Debug, Clone, Default)]
pub struct Pcm {
    pub samples: Vec<i16>,
    pub channels: u16,
    pub rate: u32,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }
}

/// One playing sound.
#[derive(Debug, Clone)]
pub struct Voice {
    pub pcm: Arc<Pcm>,
    /// Position in frames (fractional, for resampling).
    pub pos: f64,
    /// Playback rate (0: the data's own).
    pub rate: u32,
    /// Plays left: 0 forever.
    pub loops: u32,
    /// 0..1 each.
    pub volume: f32,
    /// -1 left .. 1 right.
    pub pan: f32,
    pub playing: bool,
    /// When it started, for the clock fallback.
    pub started: Instant,
    /// A streaming buffer (DirectSound): played round and round from this shared copy, which
    /// the game refills ahead of the play position.
    pub stream: Option<Arc<Mutex<Vec<i16>>>>,
}

struct Mixer {
    voices: BTreeMap<u32, Voice>,
    /// When the audio device last pulled samples.
    pulled: Option<Instant>,
}

static MIXER: Mutex<Mixer> = Mutex::new(Mixer {
    voices: BTreeMap::new(),
    pulled: None,
});

fn mx() -> std::sync::MutexGuard<'static, Mixer> {
    MIXER.lock().expect("mixer")
}

/// Start (or restart) voice `id`.
pub fn play(id: u32, pcm: Arc<Pcm>, rate: u32, loops: u32, volume: f32, pan: f32) {
    mx().voices.insert(
        id,
        Voice {
            pcm,
            pos: 0.0,
            rate,
            loops,
            volume,
            pan,
            playing: true,
            started: Instant::now(),
            stream: None,
        },
    );
}

/// Start voice `id` on a streaming buffer of `channels` channels at `rate`, looping.
pub fn play_stream(
    id: u32,
    buf: Arc<Mutex<Vec<i16>>>,
    channels: u16,
    rate: u32,
    volume: f32,
    pan: f32,
) {
    mx().voices.insert(
        id,
        Voice {
            pcm: Arc::new(Pcm {
                samples: Vec::new(),
                channels,
                rate,
            }),
            pos: 0.0,
            rate,
            loops: 0,
            volume,
            pan,
            playing: true,
            started: Instant::now(),
            stream: Some(buf),
        },
    );
}

/// The play position of a voice in frames: where the device is, or where the clock says it
/// would be without one.
pub fn position(id: u32) -> Option<f64> {
    let m = mx();
    let live = m.pulled.is_some_and(|t| t.elapsed().as_millis() < 500);
    let v = m.voices.get(&id)?;
    if live {
        Some(v.pos)
    } else {
        let rate = if v.rate != 0 {
            v.rate
        } else {
            v.pcm.rate.max(1)
        };
        Some(v.started.elapsed().as_secs_f64() * f64::from(rate))
    }
}

pub fn stop(id: u32) {
    if let Some(v) = mx().voices.get_mut(&id) {
        v.playing = false;
    }
}

pub fn remove(id: u32) {
    mx().voices.remove(&id);
}

/// Change a playing voice.
pub fn set(id: u32, f: impl FnOnce(&mut Voice)) {
    if let Some(v) = mx().voices.get_mut(&id) {
        f(v);
    }
}

/// Whether voice `id` is still playing.
pub fn playing(id: u32) -> bool {
    let mut m = mx();
    let live = m.pulled.is_some_and(|t| t.elapsed().as_millis() < 500);
    let Some(v) = m.voices.get_mut(&id) else {
        return false;
    };
    if !v.playing {
        return false;
    }
    if !live && v.loops != 0 {
        // Nothing plays the sound: let it end when its time is over.
        let rate = if v.rate != 0 {
            v.rate
        } else {
            v.pcm.rate.max(1)
        };
        let secs = v.pcm.frames() as f64 / f64::from(rate) * f64::from(v.loops);
        if v.started.elapsed().as_secs_f64() >= secs {
            v.playing = false;
        }
    }
    v.playing
}

/// The platform's audio callback: interleaved stereo at `rc_platform::audio::RATE`.
pub fn mix(out: &mut [f32]) {
    out.fill(0.0);
    let mut m = mx();
    m.pulled = Some(Instant::now());
    let out_rate = f64::from(rc_platform::audio::RATE);
    for v in m.voices.values_mut() {
        if !v.playing {
            continue;
        }
        if let Some(stream) = &v.stream {
            let buf = stream.lock().expect("stream");
            let ch = usize::from(v.pcm.channels.max(1));
            let frames = buf.len() / ch;
            if frames == 0 {
                continue;
            }
            let step = f64::from(v.rate.max(1)) / out_rate;
            for o in out.as_chunks_mut::<2>().0 {
                let i = (v.pos as usize) % frames;
                let l = f32::from(buf[i * ch]) / 32768.0;
                let r = if ch > 1 {
                    f32::from(buf[i * ch + 1]) / 32768.0
                } else {
                    l
                };
                o[0] += l * v.volume;
                o[1] += r * v.volume;
                v.pos += step;
            }
            continue;
        }
        let frames = v.pcm.frames();
        if frames == 0 {
            v.playing = false;
            continue;
        }
        let ch = usize::from(v.pcm.channels.max(1));
        let rate = if v.rate != 0 {
            v.rate
        } else {
            v.pcm.rate.max(1)
        };
        let step = f64::from(rate) / out_rate;
        let (gl, gr) = (
            v.volume * (1.0 - v.pan.max(0.0)),
            v.volume * (1.0 + v.pan.min(0.0)),
        );
        for o in out.as_chunks_mut::<2>().0 {
            let i = v.pos as usize;
            if i >= frames {
                if v.loops == 1 {
                    v.playing = false;
                    break;
                }
                if v.loops > 1 {
                    v.loops -= 1;
                }
                v.pos -= frames as f64;
                continue;
            }
            let l = f32::from(v.pcm.samples[i * ch]) / 32768.0;
            let r = if ch > 1 {
                f32::from(v.pcm.samples[i * ch + 1]) / 32768.0
            } else {
                l
            };
            o[0] += l * gl;
            o[1] += r * gr;
            v.pos += step;
        }
    }
    for s in out.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
}

// --- decoding ------------------------------------------------------------------------------

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// IMA ADPCM (Microsoft's WAV layout): blocks of `block` bytes, a header per channel.
fn ima_adpcm(data: &[u8], channels: u16, block: usize) -> Vec<i16> {
    let ch = usize::from(channels.max(1));
    let mut out = Vec::new();
    if block < 4 * ch {
        return out;
    }
    for b in data.chunks(block) {
        if b.len() < 4 * ch {
            break;
        }
        let mut pred = [0i32; 2];
        let mut idx = [0i32; 2];
        for c in 0..ch.min(2) {
            pred[c] = i32::from(i16::from_le_bytes([b[4 * c], b[4 * c + 1]]));
            idx[c] = i32::from(b[4 * c + 2]).clamp(0, 88);
        }
        let mut frames: Vec<[i16; 2]> = vec![[pred[0] as i16, pred[1] as i16]];
        // After the headers: 4 bytes (8 samples) per channel, interleaved.
        let body = &b[4 * ch..];
        let groups = body.len() / (4 * ch);
        for g in 0..groups {
            let mut chunk = [[0i16; 8]; 2];
            for c in 0..ch.min(2) {
                for k in 0..4 {
                    let byte = body[g * 4 * ch + 4 * c + k];
                    for (half, nib) in [(0, byte & 0xf), (1, byte >> 4)] {
                        let step = IMA_STEPS[idx[c] as usize];
                        let mut diff = step >> 3;
                        if nib & 1 != 0 {
                            diff += step >> 2;
                        }
                        if nib & 2 != 0 {
                            diff += step >> 1;
                        }
                        if nib & 4 != 0 {
                            diff += step;
                        }
                        if nib & 8 != 0 {
                            pred[c] -= diff;
                        } else {
                            pred[c] += diff;
                        }
                        pred[c] = pred[c].clamp(-32768, 32767);
                        idx[c] = (idx[c] + IMA_INDEX[usize::from(nib)]).clamp(0, 88);
                        chunk[c][2 * k + half] = pred[c] as i16;
                    }
                }
            }
            for i in 0..8 {
                frames.push([chunk[0][i], chunk[1][i]]);
            }
        }
        for f in frames {
            out.push(f[0]);
            if ch > 1 {
                out.push(f[1]);
            }
        }
    }
    out
}

/// Raw sample data of a format (WAVE_FORMAT tag, bits, channels) as 16-bit PCM.
pub fn decode_raw(data: &[u8], tag: u16, bits: u16, channels: u16, block: usize) -> Vec<i16> {
    match (tag, bits) {
        (0x11, _) => ima_adpcm(data, channels, block),
        (_, 8) => data.iter().map(|&b| (i16::from(b) - 128) << 8).collect(),
        _ => data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect(),
    }
}

/// A RIFF WAV file as PCM; `None` if it is not one.
pub fn decode_wav(file: &[u8]) -> Option<Pcm> {
    if file.len() < 12 || &file[0..4] != b"RIFF" || &file[8..12] != b"WAVE" {
        return None;
    }
    let mut p = 12;
    let (mut tag, mut channels, mut rate, mut bits, mut block) =
        (1u16, 1u16, 22050u32, 16u16, 0usize);
    let mut data: &[u8] = &[];
    while p + 8 <= file.len() {
        let id = &file[p..p + 4];
        let len = u32::from_le_bytes([file[p + 4], file[p + 5], file[p + 6], file[p + 7]]) as usize;
        let body = &file[p + 8..(p + 8 + len).min(file.len())];
        if id == b"fmt " && body.len() >= 16 {
            tag = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            block = usize::from(u16::from_le_bytes([body[12], body[13]]));
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            data = body;
        }
        p += 8 + len + (len & 1);
    }
    Some(Pcm {
        samples: decode_raw(data, tag, bits, channels, block),
        channels: channels.max(1),
        rate,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_wav_and_mixing() {
        let mut w = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&[1, 0, 1, 0]);
        w.extend_from_slice(&44_100u32.to_le_bytes());
        w.extend_from_slice(&88_200u32.to_le_bytes());
        w.extend_from_slice(&[2, 0, 16, 0]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&4u32.to_le_bytes());
        w.extend_from_slice(&[0x00, 0x40, 0x00, 0xc0]);
        let p = decode_wav(&w).expect("a wav");
        assert_eq!(p.samples, vec![0x4000, -0x4000]);
        play(9999, Arc::new(p), 0, 1, 1.0, 0.0);
        let mut out = [0f32; 8];
        mix(&mut out);
        assert_eq!(out[0], 0.5);
        assert_eq!(out[2], -0.5);
        assert!(!playing(9999));
        remove(9999);
    }
}

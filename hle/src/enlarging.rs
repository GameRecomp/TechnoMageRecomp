// SPDX-License-Identifier: MIT
//! Textures enlarged in the background ("smooth textures", "smooth fonts"): the renderer asks for
//! an enlarged texture, draws the plain one while it is made, and takes the enlarged one from the
//! frame it is done in (its GPU texture key differs, so it simply replaces the other).
//!
//! The work is a `Job` that can stop after any band of rows and go on later. Natively a few
//! worker threads run the jobs to their end; in a browser (no threads) they run at the end of
//! each frame for a few milliseconds (`pump`). With the virtual clock (`RC_CLOCK=virtual`,
//! scripted runs) a job is done at once, so pictures stay the same from run to run.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};

use crate::glyphs::GlyphSet;
use crate::upscale;

type Px = [u8; 4];

/// A texture's identity for the enlarged cache: object, content version, colour-keyed, variant.
pub(crate) type Key = (u32, u32, bool, u32);

/// What `Key` names apart from the version: a newer version of one replaces a waiting older one.
fn slot(k: &Key) -> (u32, bool, u32) {
    (k.0, k.2, k.3)
}

/// Output pixels per band: how much a job does between looks at the clock.
const BAND: usize = 4 * 1024;

/// Glyphs drawn per band.
const GLYPHS: usize = 8;

/// How long a frame lends to jobs where there are no worker threads.
const FRAME_BUDGET_NS: u64 = 3_000_000;

/// At most this many worker threads.
const MAX_WORKERS: usize = 4;

enum Stage {
    /// Smooth: source rows into the 2× picture.
    First,
    /// Smooth: 2× rows into the 4× picture.
    Second,
    /// Smooth: 4× rows sharpened.
    Sharpen,
    /// Not smooth: every texel taken `factor` times over.
    Nearest,
    /// Smooth, for a factor other than 4: the 4× picture taken to `factor` (nearest).
    Resample,
    /// The fonts' glyphs drawn over, a few at a time (set, first glyph), then done.
    Fonts(usize, usize),
}

/// One texture's enlargement, done in bands of rows.
pub(crate) struct Job {
    w: usize,
    h: usize,
    /// How many times larger the result is (4 when smooth unless fonts ask for another).
    factor: usize,
    /// Whether any texel is colour-keyed, handed on to `Done`.
    keyed: bool,
    src: Arc<Vec<Px>>,
    fonts: Vec<Arc<GlyphSet>>,
    stage: Stage,
    row: usize,
    /// The 2× picture (smooth), then the 4× picture.
    twice: Vec<Px>,
    four: Vec<Px>,
    /// The result: 4× and sharpened, or taken over.
    out: Vec<Px>,
}

impl Job {
    /// `w × h` texels `src` to be enlarged `factor` times, smoothed (four times, then taken to
    /// `factor`) or not, with the glyphs of `fonts` drawn over them.
    pub(crate) fn new(
        src: Arc<Vec<Px>>,
        (w, h): (usize, usize),
        factor: usize,
        keyed: bool,
        smooth: bool,
        fonts: Vec<Arc<GlyphSet>>,
    ) -> Job {
        Job {
            w,
            h,
            factor: factor.max(1),
            keyed,
            src,
            fonts,
            stage: if smooth { Stage::First } else { Stage::Nearest },
            row: 0,
            twice: Vec::new(),
            four: Vec::new(),
            out: Vec::new(),
        }
    }

    /// The next `n` rows of `0..height`, none when the stage is through.
    fn rows(&mut self, height: usize, width: usize) -> Option<Range<usize>> {
        if self.row >= height {
            self.row = 0;
            return None;
        }
        let n = (BAND / width.max(1)).max(1);
        let r = self.row..(self.row + n).min(height);
        self.row = r.end;
        Some(r)
    }

    /// Work on until done (the enlarged texels) or until `more` says to stop (`None`).
    pub(crate) fn run(&mut self, more: &mut dyn FnMut() -> bool) -> Option<Vec<Px>> {
        let (w, h) = (self.w, self.h);
        while more() {
            match self.stage {
                Stage::First => {
                    if self.twice.is_empty() {
                        self.twice = vec![[0; 4]; 4 * w * h];
                    }
                    match self.rows(h, 4 * w) {
                        Some(r) => upscale::scale2x_rows(&self.src, w, h, r, &mut self.twice),
                        None => self.stage = Stage::Second,
                    }
                }
                Stage::Second => {
                    if self.four.is_empty() {
                        self.four = vec![[0; 4]; 16 * w * h];
                    }
                    match self.rows(2 * h, 8 * w) {
                        Some(r) => {
                            upscale::scale2x_rows(&self.twice, 2 * w, 2 * h, r, &mut self.four)
                        }
                        None => {
                            self.twice = Vec::new();
                            self.out = self.four.clone();
                            self.stage = Stage::Sharpen;
                        }
                    }
                }
                Stage::Sharpen => match self.rows(4 * h, 4 * w) {
                    Some(r) => upscale::sharpen_rows(
                        &self.four,
                        4 * w,
                        4 * h,
                        upscale::SHARPNESS,
                        r,
                        &mut self.out,
                    ),
                    None => {
                        self.four = Vec::new();
                        self.stage = if self.factor == 4 {
                            Stage::Fonts(0, 0)
                        } else {
                            Stage::Resample
                        };
                    }
                },
                Stage::Resample => {
                    // The sharpened 4× picture is `out`; it becomes `factor`×, row by row.
                    let k = self.factor;
                    if self.four.is_empty() {
                        self.four = std::mem::replace(&mut self.out, vec![[0; 4]; k * k * w * h]);
                    }
                    match self.rows(k * h, k * w) {
                        Some(r) => {
                            for y in r {
                                let sy = (4 * y + 2) / k;
                                for x in 0..k * w {
                                    let sx = (4 * x + 2) / k;
                                    self.out[y * k * w + x] = self.four[sy * 4 * w + sx];
                                }
                            }
                        }
                        None => {
                            self.four = Vec::new();
                            self.stage = Stage::Fonts(0, 0);
                        }
                    }
                }
                Stage::Nearest => {
                    let k = self.factor;
                    if self.out.is_empty() {
                        self.out = vec![[0; 4]; k * k * w * h];
                    }
                    match self.rows(k * h, k * w) {
                        Some(r) => {
                            for y in r {
                                let row = &mut self.out[y * k * w..(y + 1) * k * w];
                                for (x, p) in row.iter_mut().enumerate() {
                                    *p = self.src[(y / k) * w + x / k];
                                }
                            }
                        }
                        None => self.stage = Stage::Fonts(0, 0),
                    }
                }
                Stage::Fonts(set, first) => {
                    let Some(glyphs) = self.fonts.get(set) else {
                        return Some(std::mem::take(&mut self.out));
                    };
                    let end = (first + GLYPHS).min(glyphs.cells.len());
                    crate::glyphs::refine_cells(
                        glyphs,
                        first..end,
                        &self.src,
                        w,
                        h,
                        self.factor,
                        &mut self.out,
                    );
                    self.stage = if end < glyphs.cells.len() {
                        Stage::Fonts(set, end)
                    } else {
                        Stage::Fonts(set + 1, 0)
                    };
                }
            }
        }
        None
    }
}

/// An enlarged texture made: its key, size (texels a side), keying and texels.
pub(crate) struct Done {
    pub key: Key,
    pub w: u32,
    pub h: u32,
    pub keyed: bool,
    pub texels: Vec<Px>,
}

impl Job {
    fn done(&self, key: Key, texels: Vec<Px>) -> Done {
        Done {
            key,
            w: (self.factor * self.w) as u32,
            h: (self.factor * self.h) as u32,
            keyed: self.keyed,
            texels,
        }
    }
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<(Key, Job)>,
    /// Waiting or being worked on.
    pending: HashSet<Key>,
    done: Vec<Done>,
    workers: usize,
    /// Worker threads could not be started: `pump` does the work.
    no_threads: bool,
}

static QUEUE: Mutex<Option<Queue>> = Mutex::new(None);
static WORK: Condvar = Condvar::new();

fn with<R>(f: impl FnOnce(&mut Queue) -> R) -> R {
    f(QUEUE
        .lock()
        .expect("enlarging")
        .get_or_insert_with(Default::default))
}

/// Whether `key` is being made.
pub(crate) fn pending(key: &Key) -> bool {
    with(|q| q.pending.contains(key))
}

/// The enlarged textures made since the last call.
pub(crate) fn take_done() -> Vec<Done> {
    with(|q| std::mem::take(&mut q.done))
}

/// Have `job` make `key`; a waiting job for an older version of the same texture is dropped.
pub(crate) fn submit(key: Key, mut job: Job) {
    if rc_runtime::clock::is_virtual() {
        let texels = job.run(&mut || true).expect("a job run to its end");
        with(|q| q.done.push(job.done(key, texels)));
        return;
    }
    let start = with(|q| {
        let s = slot(&key);
        let pending = &mut q.pending;
        q.waiting.retain(|(k, _)| {
            let stale = slot(k) == s;
            if stale {
                pending.remove(k);
            }
            !stale
        });
        q.pending.insert(key);
        q.waiting.push_back((key, job));
        let want = if cfg!(target_family = "wasm") || q.no_threads {
            0
        } else {
            std::thread::available_parallelism()
                .map_or(1, |n| n.get() / 2)
                .clamp(1, MAX_WORKERS)
        };
        let start = want.saturating_sub(q.workers);
        q.workers += start;
        start
    });
    for _ in 0..start {
        let spawned = std::thread::Builder::new()
            .name("enlarging".into())
            .spawn(worker);
        if spawned.is_err() {
            with(|q| {
                q.workers -= 1;
                q.no_threads = true;
            });
        }
    }
    WORK.notify_one();
}

fn worker() {
    loop {
        let (key, mut job) = {
            let mut guard = QUEUE.lock().expect("enlarging");
            loop {
                let q = guard.get_or_insert_with(Default::default);
                if let Some(next) = q.waiting.pop_front() {
                    break next;
                }
                guard = WORK.wait(guard).expect("enlarging");
            }
        };
        let texels = job.run(&mut || true).expect("a job run to its end");
        with(|q| {
            q.pending.remove(&key);
            q.done.push(job.done(key, texels));
        });
    }
}

/// Where there are no worker threads (a browser): work on the waiting jobs for a few
/// milliseconds, called once a frame.
pub(crate) fn pump() {
    if with(|q| q.workers > 0 || q.waiting.is_empty()) {
        return;
    }
    let start = rc_runtime::host::Stamp::now();
    let mut more = || start.elapsed_ns() < FRAME_BUDGET_NS;
    while let Some((key, mut job)) = with(|q| q.waiting.pop_front()) {
        match job.run(&mut more) {
            Some(texels) => with(|q| {
                q.pending.remove(&key);
                q.done.push(job.done(key, texels));
            }),
            None => {
                // Out of time: first again next frame, unless a newer version came meanwhile.
                with(|q| {
                    if q.waiting.iter().any(|(k, _)| slot(k) == slot(&key)) {
                        q.pending.remove(&key);
                    } else {
                        q.waiting.push_front((key, job));
                    }
                });
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_in_bands_makes_what_scale4x_makes() {
        let (w, h) = (37, 23);
        let src: Vec<Px> = (0..w * h)
            .map(|i| {
                let v = (i * 97 % 251) as u8;
                [v, v / 2, 255 - v, if i % 7 == 0 { 0 } else { 255 }]
            })
            .collect();
        let whole = upscale::scale4x(&src, w, h);
        let mut job = Job::new(Arc::new(src), (w, h), 4, false, true, Vec::new());
        // One band at a time.
        let mut steps = 0;
        let out = loop {
            let mut once = true;
            if let Some(out) = job.run(&mut || std::mem::replace(&mut once, false)) {
                break out;
            }
            steps += 1;
        };
        assert!(steps > 3);
        assert_eq!(out, whole);
    }

    #[test]
    fn glyphs_in_bands_are_drawn_as_at_once() {
        const FACE: &[u8] = include_bytes!("../../assets/fonts/DejaVuSans.ttf");
        // A page of 20 glyph cells, 6 × 8 each, white blocks on the key's zero.
        let (w, h) = (120usize, 8usize);
        let cells: Vec<_> = (0..20u16)
            .map(|i| crate::glyphs::Cell {
                rect: [6 * i, 0, 6, 8],
                ch: char::from(b'A' + i as u8),
            })
            .collect();
        let set = Arc::new(GlyphSet { cells, face: FACE });
        let src: Vec<Px> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if x % 6 != 0 && (1..7).contains(&y) {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 0]
                }
            })
            .collect();
        let mut at_once = Job::new(Arc::new(src.clone()), (w, h), 3, true, false, Vec::new())
            .run(&mut || true)
            .expect("done");
        crate::glyphs::refine(std::slice::from_ref(&set), &src, w, h, 3, &mut at_once);
        let mut job = Job::new(Arc::new(src), (w, h), 3, true, false, vec![set]);
        let out = loop {
            let mut once = true;
            if let Some(out) = job.run(&mut || std::mem::replace(&mut once, false)) {
                break out;
            }
        };
        assert_eq!(out, at_once);
    }
}

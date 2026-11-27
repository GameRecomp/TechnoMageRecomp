// SPDX-License-Identifier: MIT
//! A frame profiler (`RC_TRACE=profile`): the time each frame spends in named sections, written
//! as one CSV line per frame to `RC_PROFILE` (default `profile.csv`). Sections nest; each counts
//! only its own time (an inner one's is taken off the outer). What no section covers is the
//! game's own code (and the HLE's other calls): the column `game`.

use rc_runtime::host::Stamp;
use std::sync::Mutex;
use std::sync::OnceLock;

/// The sections measured.
#[derive(Clone, Copy)]
pub enum Section {
    /// The game patch's walk over the ordering table before it is drawn.
    OtWalk,
    /// A Direct3D draw recorded (converted for the GPU, or rasterized in software).
    Record,
    /// The batch of draws: the target read from guest memory into RGBA.
    FlushRead,
    /// The batch of draws: uploaded, drawn and read back on the GPU.
    FlushRender,
    /// The batch of draws: the result written back into guest memory.
    FlushWrite,
    /// In the render: the target uploaded, the vertices built, the commands encoded.
    GpuPrepare,
    /// In the render: waited for the GPU (drawing and the copy back).
    GpuWait,
    /// In the render: the result's rows unpacked, its edges filled.
    GpuUnpack,
    /// At `Flip`: the picture with native resolution (the GPU's large one, composed).
    Picture,
    /// At `Flip` and presents: the surface converted to RGBA.
    ToRgba,
    /// Over the frame: the game patch's overlay (gamepad buttons, touch buttons).
    Overlay,
    /// The frame handed to the platform.
    Submit,
    /// At `Flip`: the frame copied into the front surface.
    FrontCopy,
    /// At `Flip`: the margins cleared.
    ClearMargins,
    /// `Sleep` and waits of the game.
    Sleep,
    /// Other guest threads' turns and the host's frame yield.
    Yield,
}

const NAMES: [&str; 16] = [
    "ot_walk",
    "record",
    "flush_read",
    "flush_render",
    "flush_write",
    "gpu_prepare",
    "gpu_wait",
    "gpu_unpack",
    "picture",
    "to_rgba",
    "overlay",
    "submit",
    "front_copy",
    "clear_margins",
    "sleep",
    "yield",
];

struct State {
    /// This frame's time per section (ns), own time only.
    own: [u64; NAMES.len()],
    /// Calls per section this frame.
    calls: [u32; NAMES.len()],
    /// The sections open: which, since when, and their inner sections' time.
    open: Vec<(usize, Stamp, u64)>,
    frame_start: Option<Stamp>,
    out: Option<std::fs::File>,
}

static STATE: Mutex<State> = Mutex::new(State {
    own: [0; NAMES.len()],
    calls: [0; NAMES.len()],
    open: Vec::new(),
    frame_start: None,
    out: None,
});

/// Whether the profiler runs.
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| rc_runtime::host::env("RC_TRACE").is_some_and(|v| v.contains("profile")))
}

/// Run `f` as section `s`.
pub fn time<R>(s: Section, f: impl FnOnce() -> R) -> R {
    if !on() {
        return f();
    }
    begin(s);
    let r = f();
    end();
    r
}

/// Section `s` begins (`end` closes it): for code a closure does not fit.
pub fn begin(s: Section) {
    if !on() {
        return;
    }
    let mut st = STATE.lock().expect("profile");
    st.open.push((s as usize, Stamp::now(), 0));
}

/// The innermost open section ends.
pub fn end() {
    if !on() {
        return;
    }
    let mut st = STATE.lock().expect("profile");
    let Some((i, t, inner)) = st.open.pop() else {
        return;
    };
    let all = t.elapsed_ns();
    st.own[i] += all.saturating_sub(inner);
    st.calls[i] += 1;
    if let Some(parent) = st.open.last_mut() {
        parent.2 += all;
    }
}

/// A frame shown (frame `n`): its line written, the next one begins.
pub fn frame_end(n: u64) {
    if !on() {
        return;
    }
    use std::io::Write;
    let mut st = STATE.lock().expect("profile");
    let now = Stamp::now();
    if let Some(start) = st.frame_start {
        let total = start.elapsed_ns();
        if st.out.is_none() {
            let path = rc_runtime::host::env("RC_PROFILE").unwrap_or_else(|| "profile.csv".into());
            let mut f = std::fs::File::create(&path).ok();
            if let Some(f) = f.as_mut() {
                let mut head = String::from("frame,total_us,game_us");
                for name in NAMES {
                    head.push_str(&format!(",{name}_us"));
                }
                for name in NAMES {
                    head.push_str(&format!(",{name}_n"));
                }
                let _ = writeln!(f, "{head}");
            }
            st.out = f;
        }
        let measured: u64 = st.own.iter().sum();
        let mut line = format!(
            "{n},{},{}",
            total / 1000,
            total.saturating_sub(measured) / 1000
        );
        for v in st.own {
            line.push_str(&format!(",{}", v / 1000));
        }
        for v in st.calls {
            line.push_str(&format!(",{v}"));
        }
        if let Some(f) = st.out.as_mut() {
            let _ = writeln!(f, "{line}");
        }
    }
    st.own = [0; NAMES.len()];
    st.calls = [0; NAMES.len()];
    st.frame_start = Some(now);
}

/// Move `ns` of section `from`'s time this frame to section `to` (a part measured inside it).
pub fn shift(from: Section, to: Section, ns: u64) {
    if !on() {
        return;
    }
    let mut st = STATE.lock().expect("profile");
    let ns = ns.min(st.own[from as usize]);
    st.own[from as usize] -= ns;
    st.own[to as usize] += ns;
    st.calls[to as usize] += 1;
}

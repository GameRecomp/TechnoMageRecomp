// SPDX-License-Identifier: MIT
//! Android (tools/build-android): the app is a NativeActivity running this program as
//! `libtechnomage.so`. A converter package carries the game's code and files as assets under
//! `assets/data/` with an index, `files.txt` (a line per entry: its size and path, directories
//! ending in `/`). On the first start, and after an update brings a different index, they are
//! unpacked into the app's own storage, where the shell finds them as beside a desktop program.

use bevy::android::android_activity::AndroidApp;
use std::ffi::CString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub use bevy::android::android_activity;

/// Keep the app handle for Bevy (its winit backend takes it from here) and for us; the screen
/// stays on while the game runs, and the game has it whole (no system bars).
pub fn init(app: AndroidApp) {
    use android_activity::WindowManagerFlags as F;
    app.set_window_flags(F::KEEP_SCREEN_ON | F::FULLSCREEN, F::empty());
    let _ = bevy::android::ANDROID_APP.set(app);
    log_to_file();
    immersive();
}

/// Hide the system bars (the status bar, the navigation bar or a launcher's task bar, as some
/// Samsung devices show one) in "sticky immersive" mode: a swipe from the edge shows them for a
/// moment. Android drops the mode when the app loses the focus, so it is asked again when the
/// window has it back. Runs on Java's main thread, as views must be changed there.
pub fn immersive() {
    let Some(app) = app() else {
        return;
    };
    let handle = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        if let Err(e) = try_immersive(&handle) {
            eprintln!("android: system bars not hidden: {e}");
        }
    }));
}

fn try_immersive(app: &AndroidApp) -> jni::errors::Result<()> {
    use jni::objects::{JObject, JValue};
    // View.SYSTEM_UI_FLAG_…: LAYOUT_STABLE, LAYOUT_HIDE_NAVIGATION, LAYOUT_FULLSCREEN,
    // HIDE_NAVIGATION, FULLSCREEN, IMMERSIVE_STICKY.
    const FLAGS: i32 = 0x100 | 0x200 | 0x400 | 0x2 | 0x4 | 0x1000;
    // WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES (Android 9).
    const SHORT_EDGES: i32 = 1;
    // SAFETY: the activity's Java VM, which lives as long as the process.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast())? };
    let mut env = vm.attach_current_thread_permanently()?;
    let result = env.with_local_frame(16, |env| -> jni::errors::Result<()> {
        // SAFETY: the activity's global reference, valid while the app runs.
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        let window = env
            .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let attrs = env
            .call_method(
                &window,
                "getAttributes",
                "()Landroid/view/WindowManager$LayoutParams;",
                &[],
            )?
            .l()?;
        if env
            .set_field(
                &attrs,
                "layoutInDisplayCutoutMode",
                "I",
                JValue::Int(SHORT_EDGES),
            )
            .is_ok()
        {
            env.call_method(
                &window,
                "setAttributes",
                "(Landroid/view/WindowManager$LayoutParams;)V",
                &[JValue::Object(&attrs)],
            )?;
        } else if env.exception_check()? {
            // Before Android 9: no such field.
            env.exception_clear()?;
        }
        let view = env
            .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
            .l()?;
        env.call_method(
            &view,
            "setSystemUiVisibility",
            "(I)V",
            &[JValue::Int(FLAGS)],
        )?;
        Ok(())
    });
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result
}

fn app() -> Option<&'static AndroidApp> {
    bevy::android::ANDROID_APP.get()
}

/// The app's private directory (`/data/data/<package>/files`).
pub fn internal_dir() -> Option<PathBuf> {
    app()?.internal_data_path()
}

/// stderr (where the runtime logs) into `log.txt` in the app's external files directory
/// (`Android/data/<package>/files`, which a computer sees over USB): Android drops it otherwise.
fn log_to_file() {
    unsafe extern "C" {
        fn dup2(old: i32, new: i32) -> i32;
    }
    use std::os::fd::IntoRawFd;
    let Some(dir) = app()
        .and_then(AndroidApp::external_data_path)
        .or_else(internal_dir)
    else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(f) = std::fs::File::create(dir.join("log.txt")) {
        let fd = f.into_raw_fd();
        // SAFETY: both descriptors are open; stderr now writes to the file (which stays open).
        unsafe {
            dup2(fd, 2);
        }
    }
}

/// An asset's whole content (`path` below `assets/`).
fn asset(path: &str) -> Option<ndk::asset::Asset> {
    let name = CString::new(path).ok()?;
    app()?.asset_manager().open(&name)
}

use bevy::android::android_activity::ndk;

/// The package's data directory (`data/` of a desktop package): unpacked from the assets if
/// they are newer than what was unpacked before. `None` without bundled data (a shell APK
/// without a game).
pub fn bundled_data() -> Option<PathBuf> {
    let mut index = String::new();
    asset("data/files.txt")?.read_to_string(&mut index).ok()?;
    let root = internal_dir()?.join("data");
    let stamp = root.join("files.txt");
    if std::fs::read_to_string(&stamp).ok().as_deref() == Some(index.as_str()) {
        return Some(root);
    }
    eprintln!("android: unpacking the game data to {}", root.display());
    // The index last, so an interrupted unpacking is done again on the next start.
    let _ = std::fs::remove_file(&stamp);
    let total: u64 = index
        .lines()
        .filter_map(|l| l.split_once(' ')?.0.parse::<u64>().ok())
        .sum();
    let mut done = 0u64;
    let mut shown = 0;
    for line in index.lines() {
        let Some((size, path)) = line.split_once(' ') else {
            continue;
        };
        if path.contains("..") {
            continue;
        }
        let dest = root.join(path);
        if path.ends_with('/') {
            if let Err(e) = std::fs::create_dir_all(&dest) {
                eprintln!("android: {}: {e}", dest.display());
                return None;
            }
            continue;
        }
        if let Err(e) = unpack(&format!("data/{path}"), &dest) {
            eprintln!("android: {path}: {e}");
            return None;
        }
        done += size.parse::<u64>().unwrap_or(0);
        let percent = (done * 100).checked_div(total).unwrap_or(100);
        if percent >= shown + 10 {
            shown = percent;
            eprintln!("android: {percent} % unpacked");
        }
    }
    std::fs::write(&stamp, &index).ok()?;
    Some(root)
}

/// One asset into a file, replaced whole (an earlier copy is truncated first).
fn unpack(asset_path: &str, dest: &Path) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut a = asset(asset_path).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "not in the package's assets")
    })?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(dest)?);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = a.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
    }
    out.flush()
}

// --- Gamepads ----------------------------------------------------------------------------------
//
// Android has no API to ask a gamepad's state: its buttons and axes come as input events, on the
// one input queue of the NativeActivity, which Bevy's window (winit, through android-activity)
// reads. winit passes a gamepad's buttons on as unknown keys and turns its sticks into touches,
// losing the axes. So the queue's events are looked at before winit gets them: android-activity
// takes each one with the NDK's `AInputQueue_getEvent`, and this library defines that function
// itself, which the linker binds android-activity's call to. Ours calls the system's (from
// libandroid.so), keeps the events of gamepads and joysticks (finishing them as handled) and
// hands every other one on. Should android-activity ever read the queue otherwise, the gamepads
// simply stay silent on Android, as before.

use std::ffi::c_void;
use std::sync::Mutex;

type GetEvent = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32;

#[link(name = "android")]
unsafe extern "C" {
    fn AInputEvent_getType(event: *const c_void) -> i32;
    fn AInputEvent_getSource(event: *const c_void) -> i32;
    fn AInputEvent_getDeviceId(event: *const c_void) -> i32;
    fn AKeyEvent_getAction(event: *const c_void) -> i32;
    fn AKeyEvent_getKeyCode(event: *const c_void) -> i32;
    fn AMotionEvent_getAxisValue(event: *const c_void, axis: i32, pointer: usize) -> f32;
    fn AInputQueue_finishEvent(queue: *mut c_void, event: *mut c_void, handled: i32);
}

unsafe extern "C" {
    fn dlopen(name: *const std::ffi::c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
}

/// The system's `AInputQueue_getEvent`, from libandroid.so.
fn system_get_event() -> Option<GetEvent> {
    static F: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    let f = *F.get_or_init(|| {
        const RTLD_NOW: i32 = 2;
        // SAFETY: plain C strings; dlopen and dlsym only read them.
        let h = unsafe { dlopen(c"libandroid.so".as_ptr(), RTLD_NOW) };
        if h.is_null() {
            return None;
        }
        // SAFETY: as above; the handle is libandroid's.
        let p = unsafe { dlsym(h, c"AInputQueue_getEvent".as_ptr()) };
        (!p.is_null()).then_some(p as usize)
    });
    // SAFETY: the address is libandroid's AInputQueue_getEvent, of this signature.
    f.map(|p| unsafe { std::mem::transmute::<usize, GetEvent>(p) })
}

const AINPUT_EVENT_TYPE_KEY: i32 = 1;
const AINPUT_EVENT_TYPE_MOTION: i32 = 2;
/// `AINPUT_SOURCE_GAMEPAD` (its class bits left out) and `AINPUT_SOURCE_JOYSTICK`'s.
const SOURCE_GAMEPAD: i32 = 0x0000_0400;
const SOURCE_JOYSTICK: i32 = 0x0100_0000;
const AKEY_EVENT_ACTION_DOWN: i32 = 0;
const AKEY_EVENT_ACTION_UP: i32 = 1;

/// A gamepad's key (`AKEYCODE_*`): a button of the pad, or a direction (left, right, up, down).
enum Key {
    Button(u16),
    Direction(usize),
}

fn key(code: i32) -> Option<Key> {
    use crate::pad;
    Some(match code {
        96 => Key::Button(pad::CROSS),     // BUTTON_A
        97 => Key::Button(pad::CIRCLE),    // BUTTON_B
        99 => Key::Button(pad::SQUARE),    // BUTTON_X
        100 => Key::Button(pad::TRIANGLE), // BUTTON_Y
        102 => Key::Button(pad::L1),
        103 => Key::Button(pad::R1),
        104 => Key::Button(pad::L2),
        105 => Key::Button(pad::R2),
        106 => Key::Button(pad::L3),     // BUTTON_THUMBL
        107 => Key::Button(pad::R3),     // BUTTON_THUMBR
        108 => Key::Button(pad::START),  // BUTTON_START
        109 => Key::Button(pad::SELECT), // BUTTON_SELECT
        21 => Key::Direction(0),         // DPAD_LEFT
        22 => Key::Direction(1),         // DPAD_RIGHT
        19 => Key::Direction(2),         // DPAD_UP
        20 => Key::Direction(3),         // DPAD_DOWN
        _ => return None,
    })
}

/// What the gamepads hold: buttons by key, the directional keys, and the axes (the left stick,
/// the hat, the triggers).
#[derive(Default)]
struct Pads {
    buttons: u16,
    directions: [bool; 4],
    stick: (f32, f32),
    hat: (f32, f32),
    triggers: (f32, f32),
}

static PADS: Mutex<Pads> = Mutex::new(Pads {
    buttons: 0,
    directions: [false; 4],
    stick: (0.0, 0.0),
    hat: (0.0, 0.0),
    triggers: (0.0, 0.0),
});

/// The pads to the game: the buttons (the triggers pressed past half), the direction from the
/// directional keys, else the hat, else the stick.
fn send(p: &Pads) {
    use crate::pad;
    // A gamepad in use: the on-screen buttons go.
    crate::touch::set_in_use(false);
    let mut buttons = p.buttons;
    if p.triggers.0 > 0.5 {
        buttons |= pad::L2;
    }
    if p.triggers.1 > 0.5 {
        buttons |= pad::R2;
    }
    let d = |neg: bool, pos: bool| f32::from(u8::from(pos)) - f32::from(u8::from(neg));
    let keys = (
        d(p.directions[0], p.directions[1]),
        d(p.directions[2], p.directions[3]),
    );
    let (x, y) = if keys != (0.0, 0.0) {
        keys
    } else if p.hat != (0.0, 0.0) {
        p.hat
    } else {
        p.stick
    };
    let axis = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
    // Android's y points down, as DirectInput's.
    crate::input(crate::Input::Pad {
        buttons,
        x: axis(x),
        y: axis(y),
    });
}

/// Take a gamepad's or joystick's event into [`PADS`]; whether it was one.
///
/// # Safety
///
/// `event` is an input event the queue gave out and not yet finished.
unsafe fn take(event: *mut c_void) -> bool {
    // SAFETY: the caller's event.
    let source = unsafe { AInputEvent_getSource(event) };
    if source & (SOURCE_GAMEPAD | SOURCE_JOYSTICK) == 0 {
        return false;
    }
    // Whose buttons: the maker of the device (its vendor id, asked once per device).
    // SAFETY: the caller's event.
    let device = unsafe { AInputEvent_getDeviceId(event) };
    if let Some(v) = vendor_of(device) {
        crate::pad::set_style(crate::pad::style_of_vendor(v));
    }
    let mut p = PADS.lock().expect("pads");
    // SAFETY: the caller's event, of the type asked.
    match unsafe { AInputEvent_getType(event) } {
        AINPUT_EVENT_TYPE_KEY => {
            // SAFETY: a key event.
            let (code, action) =
                unsafe { (AKeyEvent_getKeyCode(event), AKeyEvent_getAction(event)) };
            let Some(k) = key(code) else {
                return false;
            };
            let down = match action {
                AKEY_EVENT_ACTION_DOWN => true,
                AKEY_EVENT_ACTION_UP => false,
                _ => return true,
            };
            match k {
                Key::Button(b) if down => p.buttons |= b,
                Key::Button(b) => p.buttons &= !b,
                Key::Direction(i) => p.directions[i] = down,
            }
        }
        AINPUT_EVENT_TYPE_MOTION => {
            // AMOTION_EVENT_AXIS_X, _Y, _HAT_X, _HAT_Y, _LTRIGGER, _RTRIGGER, _BRAKE, _GAS.
            // SAFETY: a motion event; pointer 0 is there for joysticks.
            let a = |axis: i32| unsafe { AMotionEvent_getAxisValue(event, axis, 0) };
            p.stick = (a(0), a(1));
            p.hat = (a(15), a(16));
            p.triggers = (a(17).max(a(23)), a(18).max(a(22)));
        }
        _ => return false,
    }
    send(&p);
    true
}

/// The NDK's `AInputQueue_getEvent` for android-activity: the next event that is not a
/// gamepad's (those are taken here).
///
/// # Safety
///
/// As the NDK's: `queue` is the activity's input queue, `out` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AInputQueue_getEvent(queue: *mut c_void, out: *mut *mut c_void) -> i32 {
    /// `-EAGAIN`: no event (what the system's gives then).
    const NO_EVENT: i32 = -11;
    let Some(get) = system_get_event() else {
        return NO_EVENT;
    };
    loop {
        // SAFETY: the caller's queue and out pointer.
        let r = unsafe { get(queue, out) };
        if r < 0 {
            return r;
        }
        // SAFETY: the event the queue just gave out.
        let event = unsafe { *out };
        // SAFETY: as above; a taken event is finished here, as handled.
        if event.is_null() || !unsafe { take(event) } {
            return r;
        }
        // SAFETY: the queue's own event, finished once.
        unsafe { AInputQueue_finishEvent(queue, event, 1) };
    }
}

/// An input device's USB vendor id (`InputDevice.getVendorId()`, through JNI), kept per device.
fn vendor_of(device: i32) -> Option<u16> {
    static KNOWN: Mutex<Vec<(i32, Option<u16>)>> = Mutex::new(Vec::new());
    if let Some(&(_, v)) = KNOWN
        .lock()
        .expect("vendors")
        .iter()
        .find(|(d, _)| *d == device)
    {
        return v;
    }
    let v = ask_vendor(device).ok().flatten();
    KNOWN.lock().expect("vendors").push((device, v));
    v
}

fn ask_vendor(device: i32) -> jni::errors::Result<Option<u16>> {
    use jni::objects::JValue;
    let Some(app) = app() else {
        return Ok(None);
    };
    // SAFETY: the activity's Java VM, which lives as long as the process.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast())? };
    let mut env = vm.attach_current_thread_permanently()?;
    let r = env.with_local_frame(8, |env| -> jni::errors::Result<Option<u16>> {
        let dev = env
            .call_static_method(
                "android/view/InputDevice",
                "getDevice",
                "(I)Landroid/view/InputDevice;",
                &[JValue::Int(device)],
            )?
            .l()?;
        if dev.is_null() {
            return Ok(None);
        }
        let v = env.call_method(&dev, "getVendorId", "()I", &[])?.i()?;
        Ok(u16::try_from(v).ok())
    });
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    r
}

// --- The gamepads' vibration -------------------------------------------------------------------

/// The gamepads vibrate with `strength` (0 to 1: stop) for `duration`, through Android's
/// `InputDevice.getVibrator()` (each gamepad's or joystick's own) and `VibrationEffect`.
pub fn vibrate(strength: f32, duration: std::time::Duration) {
    static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if let Err(e) = try_vibrate(strength, duration) {
        if !FAILED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("android: no gamepad vibration: {e}");
        }
    }
}

fn try_vibrate(strength: f32, duration: std::time::Duration) -> jni::errors::Result<()> {
    use jni::objects::{JIntArray, JObject, JValue};
    let Some(app) = app() else {
        return Ok(());
    };
    // SAFETY: the activity's Java VM, which lives as long as the process.
    let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast())? };
    let mut env = vm.attach_current_thread_permanently()?;
    let result = env.with_local_frame(64, |env| -> jni::errors::Result<()> {
        let ids: JIntArray = env
            .call_static_method("android/view/InputDevice", "getDeviceIds", "()[I", &[])?
            .l()?
            .into();
        let mut list = vec![0i32; env.get_array_length(&ids)? as usize];
        env.get_int_array_region(&ids, 0, &mut list)?;
        for id in list {
            let dev = env
                .call_static_method(
                    "android/view/InputDevice",
                    "getDevice",
                    "(I)Landroid/view/InputDevice;",
                    &[JValue::Int(id)],
                )?
                .l()?;
            if dev.is_null() {
                continue;
            }
            let sources = env.call_method(&dev, "getSources", "()I", &[])?.i()?;
            if sources & SOURCE_GAMEPAD == 0 && sources & SOURCE_JOYSTICK == 0 {
                continue;
            }
            let vib = env
                .call_method(&dev, "getVibrator", "()Landroid/os/Vibrator;", &[])?
                .l()?;
            if vib.is_null() || !env.call_method(&vib, "hasVibrator", "()Z", &[])?.z()? {
                continue;
            }
            if strength <= 0.0 {
                env.call_method(&vib, "cancel", "()V", &[])?;
                continue;
            }
            let amplitude = ((strength * 255.0).round() as i32).clamp(1, 255);
            let effect: JObject = env
                .call_static_method(
                    "android/os/VibrationEffect",
                    "createOneShot",
                    "(JI)Landroid/os/VibrationEffect;",
                    &[
                        JValue::Long(duration.as_millis() as i64),
                        JValue::Int(amplitude),
                    ],
                )?
                .l()?;
            env.call_method(
                &vib,
                "vibrate",
                "(Landroid/os/VibrationEffect;)V",
                &[JValue::Object(&effect)],
            )?;
        }
        Ok(())
    });
    // A Java exception (a device gone meanwhile) is cleared, not left pending.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    result
}
